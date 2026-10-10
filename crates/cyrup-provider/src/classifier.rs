//! The classifier model type: the slice of pi 0.99's multi-type model surface that `classify`
//! needs (`packages/ai/src/types.ts`, `models.ts`, `utils/model-operations.ts` @v0.99.2-17).
//!
//! pi 0.99 widened a provider's catalog from "chat models" to "any model" (`AnyModel`, types.ts:1168)
//! and added a second one-shot operation beside image generation: `classify`, which reads a model's
//! next-token label probabilities instead of generating text. This module holds everything that
//! operation is made of:
//!
//! - [`ModelType`] / [`AnyModel`] / [`ImageModel`] / [`ClassifierModel`] — what a catalog entry is
//!   for (`ModelTypeMap`, types.ts:1158-1162; `ImageModel`, :1145-1149; `ClassifierModel`,
//!   :1152-1155). PROV-128 made the union three-valued at v1.0.0 (`a328aa89a`).
//! - [`KnownClassifierApi`] — the classifier wire-protocol ids (types.ts:35).
//! - [`ClassifierContext`] / [`ClassifierQuestion`] — the request (types.ts:633-656).
//! - [`ClassifierResult`] / [`ClassifierAnswer`] — the response (types.ts:658-688).
//! - [`ClassifierOptions`] — per-request options (types.ts:324-331 over `ProviderRequestOptions`,
//!   :132-185).
//! - [`ProviderClassifier`] — the contract a classifier api implements (types.ts:315-322).
//! - [`ClassifierApiRegistry`] — pi's per-provider `classifiers` map (models.ts:1022-1023) plus the
//!   composer's extension-first dispatch (`provider-composer.ts:652-664`).
//!
//! Dispatch itself lives where pi keeps it: [`crate::provider::Provider::classify`] and
//! [`crate::collection::Models::classify`].
//!
//! Not ported from the 0.99 multi-type surface (see the EXT-027 ledger rows): the array-based
//! `models.all.json` shape. PROV-104 ported the two System One classifier apis,
//! `typesafe-system-one` and `cloudflare-workers-ai-system-one` ([`KnownClassifierApi`]).
//!
//! PROV-128 is CLOSED. Step (1) is the type work here: [`ModelType::Image`], [`AnyModel::Image`]
//! and [`ImageModel`] in upstream's v1.0.0 shape. Step (2) hung an `images` dispatch map on
//! [`crate::provider::Provider`] ([`crate::images::ImageApiRegistry`],
//! [`crate::wire::WireProvider::with_images`]) and added `generate_images` to
//! [`crate::collection::Models`]; the catalog path constructs [`AnyModel::Image`] for real
//! ([`crate::remote_catalog`] asks pi.dev for `?types=chat,image,classifier`). Step (3) retired
//! the parallel `ImagesProvider`/`ImagesModels` tree the way upstream did — [`crate::images`] now
//! holds only what `types.ts` still holds.

use crate::HeaderMap;
use crate::auth::ProviderEnv;
use crate::model::{Modality, Model, ModelCost, ModelInputLimits};
use crate::stream::{ProviderResponse, TransformHeadersFn};
use cyrup_core::{ApiId, CancelToken, Content, ModelId, ProviderId, Usage};
use std::collections::BTreeMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------------- model types --

/// What a catalog entry is for; decides which `Models` operation accepts it (pi `ModelType =
/// keyof ModelTypeMap`, types.ts:1165).
///
/// PROV-128 — `a328aa89a` ("unify image and classifier models") made this three-valued at
/// v1.0.0: `ModelTypeMap` is `{ chat: Model<Api>; image: ImageModel<ImageApi>; classifier:
/// ClassifierModel<ClassifierApi> }` (`types.ts:1158-1162`), and the parallel images registry
/// (`images-models.ts`, `providers/openrouter-images.ts`, `builtinImagesProviders`,
/// `builtinImagesModels`) was deleted in the same commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelType {
    Chat,
    Image,
    Classifier,
}

impl ModelType {
    /// Every type this build knows (pi `KNOWN_MODEL_TYPES`, models.ts:116). Declaration order is
    /// upstream's `["chat", "image", "classifier"]`.
    pub const ALL: [ModelType; 3] = [ModelType::Chat, ModelType::Image, ModelType::Classifier];

    /// The wire spelling (pi `model.type`, with `chat` being the omitted default).
    pub const fn as_str(self) -> &'static str {
        match self {
            ModelType::Chat => "chat",
            ModelType::Image => "image",
            ModelType::Classifier => "classifier",
        }
    }
}

impl std::fmt::Display for ModelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The classifier wire-protocol ids this build knows (pi `KnownClassifierApi`, types.ts:78-82
/// @f1b2e77f5). pi's fourth, `openai-decisions`, is not ported (filed separately). Like pi's
/// `ClassifierApi = KnownClassifierApi | (string & {})`, a [`ClassifierModel::api`] may carry any
/// string; this enum names the ones with an implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnownClassifierApi {
    /// TypeSafe's native System One protocol, served by TypeSafe, OpenRouter and llama.cpp's
    /// decision models (`typesafe-system-one`, `ai/src/api/typesafe-system-one.ts`; PROV-104).
    TypesafeSystemOne,
    /// System One models on Cloudflare's Workers AI REST endpoint
    /// (`cloudflare-workers-ai-system-one`, `ai/src/api/cloudflare-workers-ai-system-one.ts`;
    /// PROV-104).
    CloudflareWorkersAiSystemOne,
    /// Classification with a chat model served by llama.cpp's `llama-server`
    /// (`llama-cpp-classify`, `ai/src/api/llama-cpp-classify.ts`).
    LlamaCppClassify,
}

impl KnownClassifierApi {
    /// pi's declaration order (types.ts:78-82), without `openai-decisions`.
    pub const ALL: [KnownClassifierApi; 3] = [
        KnownClassifierApi::TypesafeSystemOne,
        KnownClassifierApi::CloudflareWorkersAiSystemOne,
        KnownClassifierApi::LlamaCppClassify,
    ];

    /// The api id as it appears in [`ClassifierModel::api`].
    pub const fn as_str(self) -> &'static str {
        match self {
            KnownClassifierApi::TypesafeSystemOne => "typesafe-system-one",
            KnownClassifierApi::CloudflareWorkersAiSystemOne => "cloudflare-workers-ai-system-one",
            KnownClassifierApi::LlamaCppClassify => "llama-cpp-classify",
        }
    }

    /// The known api an id names, if any.
    pub fn from_api(api: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == api)
    }

    /// The implementation of this api (pi's `*Api()` factories in `api/*.lazy.ts`).
    pub fn implementation(self) -> Arc<dyn ProviderClassifier> {
        match self {
            KnownClassifierApi::TypesafeSystemOne => {
                crate::api::typesafe_system_one::typesafe_system_one_api()
            }
            KnownClassifierApi::CloudflareWorkersAiSystemOne => {
                crate::api::cloudflare_workers_ai_system_one::cloudflare_workers_ai_system_one_api()
            }
            KnownClassifierApi::LlamaCppClassify => {
                crate::api::llama_cpp_classify::llama_cpp_classify_api()
            }
        }
    }
}

impl std::fmt::Display for KnownClassifierApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<KnownClassifierApi> for ApiId {
    fn from(api: KnownClassifierApi) -> Self {
        ApiId::from(api.as_str())
    }
}

/// Structured classifier model: usable with `classify()` only (pi `ClassifierModel`,
/// types.ts:1152-1155 over `BaseModel`, :1097-1108). The `type` member is always `"classifier"` on
/// the wire, which is what tells it apart from a chat [`Model`] inside one array; it is required
/// on read, so a chat model's JSON is not accepted as a classifier model.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename = "classifier", rename_all = "camelCase")]
pub struct ClassifierModel {
    pub id: ModelId,
    pub name: String,
    pub api: ApiId,
    pub provider: ProviderId,
    pub base_url: String,
    pub input: Vec<Modality>,
    /// Provider input limits and cache-safe image preprocessing metadata (pi
    /// `BaseModel.inputLimits`, `types.ts:1105` @v1.0.4, declared between `input` and `cost`).
    /// Declared on `BaseModel`, which this type extends, so an classifier row carries it exactly as a
    /// chat [`Model`] does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,
    pub cost: ModelCost,
    /// Top-level per-provider request headers (pi `BaseModel.headers`, types.ts:1107).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderMap>,
    pub context_window: u64,
}

/// The `type` member of a [`ClassifierModel`]; its only value is `classifier`.
#[derive(serde::Deserialize)]
enum ClassifierTag {
    #[serde(rename = "classifier")]
    Classifier,
}

/// [`ClassifierModel`] as read: serde's struct-level `tag` only writes the member, so reading goes
/// through this mirror to make the tag mandatory.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClassifierModelWire {
    #[serde(rename = "type")]
    _kind: ClassifierTag,
    id: ModelId,
    name: String,
    api: ApiId,
    provider: ProviderId,
    base_url: String,
    input: Vec<Modality>,
    // Mirrors `ClassifierModel::input_limits`. A field added to the public struct but FORGOTTEN
    // here compiles clean, serializes correctly and silently reads `None` on every row — the
    // exact silent-data-loss mode these two hand-written mirrors invite.
    #[serde(default)]
    input_limits: Option<ModelInputLimits>,
    cost: ModelCost,
    #[serde(default)]
    headers: Option<HeaderMap>,
    context_window: u64,
}

impl<'de> serde::Deserialize<'de> for ClassifierModel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ClassifierModelWire::deserialize(deserializer)?;
        Ok(Self {
            id: wire.id,
            name: wire.name,
            api: wire.api,
            provider: wire.provider,
            base_url: wire.base_url,
            input: wire.input,
            input_limits: wire.input_limits,
            cost: wire.cost,
            headers: wire.headers,
            context_window: wire.context_window,
        })
    }
}

impl ClassifierModel {
    /// The chat-[`Model`] shim auth resolution operates on: pi types `resolveProviderAuth` for
    /// every model shape, cyrup's takes a [`Model`], and strategies read only identity, `base_url`
    /// and `headers` from it (the same adapter [`ImageModel::to_auth_model`] uses). The
    /// chat-only members default to non-reasoning / zero output.
    pub(crate) fn to_auth_model(&self) -> Model {
        Model {
            id: self.id.clone(),
            name: self.name.clone(),
            api: self.api.clone(),
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            reasoning: false,
            input: self.input.clone(),
            // Carried, not dropped: upstream's `resolveProviderAuth` receives the REAL model, so
            // it sees `inputLimits`. The chat-only members below are `None`/`0` because an image
            // or classifier row genuinely has no source for them; this one has one.
            input_limits: self.input_limits.clone(),
            cost: self.cost.clone(),
            prompt_cache: None,
            context_window: self.context_window,
            max_tokens: 0,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: self.headers.clone(),
        }
    }
}

/// Image-generation model: usable with `generateImages()` only (pi `ImageModel`,
/// types.ts:1145-1149 over `BaseModel`, :1097-1108).
///
/// PROV-128 — `a328aa89a` folded image models onto the one provider surface, so an image row is a
/// [`BaseModel`](Model) plus a required `type: "image"` discriminant and a required `output`
/// modality list. Two shape notes against the v0.87.1-era `ImagesModel` this replaced (PROV-128
/// step (3) deleted it, as `a328aa89a` deleted upstream's):
///
/// * `thinkingLevelMap` is **gone**. At v0.87.1 `ImagesModel` extended `Model` and inherited it;
///   v1.0.0's `ImageModel` extends `BaseModel`, which has no such member (`types.ts:1097-1108`),
///   and the field was never read on an image path.
/// * `type` is required on read, the same way [`ClassifierModel`]'s is, so a chat model's JSON is
///   not accepted as an image model inside one mixed array.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename = "image", rename_all = "camelCase")]
pub struct ImageModel {
    pub id: ModelId,
    pub name: String,
    pub api: ApiId,
    pub provider: ProviderId,
    pub base_url: String,
    pub input: Vec<Modality>,
    /// Provider input limits and cache-safe image preprocessing metadata (pi
    /// `BaseModel.inputLimits`, `types.ts:1105` @v1.0.4, declared between `input` and `cost`).
    /// Declared on `BaseModel`, which this type extends, so an image row carries it exactly as a
    /// chat [`Model`] does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,

    /// Output modalities. Always includes `image`; `text` means the model can also return text
    /// blocks (pi `ImageModel.output`, types.ts:1148).
    pub output: Vec<Modality>,
    pub cost: ModelCost,
    /// Top-level per-provider request headers (pi `BaseModel.headers`, types.ts:1107).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderMap>,
}

/// The `type` member of an [`ImageModel`]; its only value is `image`.
#[derive(serde::Deserialize)]
enum ImageTag {
    #[serde(rename = "image")]
    Image,
}

/// [`ImageModel`] as read: serde's struct-level `tag` only writes the member, so reading goes
/// through this mirror to make the tag mandatory (same reason as `ClassifierModelWire`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageModelWire {
    #[serde(rename = "type")]
    _kind: ImageTag,
    id: ModelId,
    name: String,
    api: ApiId,
    provider: ProviderId,
    base_url: String,
    input: Vec<Modality>,
    // Mirrors `ImageModel::input_limits` — see the note on `ClassifierModelWire`.
    #[serde(default)]
    input_limits: Option<ModelInputLimits>,
    output: Vec<Modality>,
    cost: ModelCost,
    #[serde(default)]
    headers: Option<HeaderMap>,
}

impl<'de> serde::Deserialize<'de> for ImageModel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ImageModelWire::deserialize(deserializer)?;
        Ok(Self {
            id: wire.id,
            name: wire.name,
            api: wire.api,
            provider: wire.provider,
            base_url: wire.base_url,
            input: wire.input,
            input_limits: wire.input_limits,
            output: wire.output,
            cost: wire.cost,
            headers: wire.headers,
        })
    }
}

impl ImageModel {
    /// `true` if this model emits text alongside images (pi `model.output.includes("text")`,
    /// `api/openrouter-images.ts:149`).
    pub fn outputs_text(&self) -> bool {
        self.output.contains(&Modality::Text)
    }

    /// The chat-[`Model`] shim auth resolution operates on, the twin of
    /// [`ClassifierModel::to_auth_model`]: pi types `resolveProviderAuth` for every model shape,
    /// cyrup's takes a [`Model`], and strategies read only identity, `base_url` and `headers` from
    /// it. The chat-only members default to non-reasoning / zero window / zero output — an image
    /// row genuinely has none of them (pi's `ImageModel` extends `BaseModel`, which declares
    /// neither `contextWindow` nor `maxTokens`, `types.ts:1097-1108`), which is why this is a
    /// private shim for one caller rather than a `From` impl.
    pub(crate) fn to_auth_model(&self) -> Model {
        Model {
            id: self.id.clone(),
            name: self.name.clone(),
            api: self.api.clone(),
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            reasoning: false,
            input: self.input.clone(),
            // Carried, not dropped: upstream's `resolveProviderAuth` receives the REAL model, so
            // it sees `inputLimits`. The chat-only members below are `None`/`0` because an image
            // or classifier row genuinely has no source for them; this one has one.
            input_limits: self.input_limits.clone(),
            cost: self.cost.clone(),
            prompt_cache: None,
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: self.headers.clone(),
        }
    }
}

/// Anything a provider can list (pi `AnyModel = ModelTypeMap[ModelType]`, types.ts:1168). Narrow
/// with [`AnyModel::as_chat`] / [`AnyModel::as_classifier`] or match on the variant (pi's
/// `isModelType()`).
//
// A short-lived listing value (a provider hands out a fresh `Vec<AnyModel>` per read, and every
// caller narrows it at once); boxing the chat variant would put a `Box` in every constructor and
// match of the chat path for nothing measurable.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum AnyModel {
    Chat(Model),
    Image(ImageModel),
    Classifier(ClassifierModel),
}

impl AnyModel {
    /// pi `getModelType` (model-operations.ts:17-19): a chat model is the default.
    pub fn model_type(&self) -> ModelType {
        match self {
            AnyModel::Chat(_) => ModelType::Chat,
            AnyModel::Image(_) => ModelType::Image,
            AnyModel::Classifier(_) => ModelType::Classifier,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            AnyModel::Chat(m) => m.id.as_str(),
            AnyModel::Image(m) => m.id.as_str(),
            AnyModel::Classifier(m) => m.id.as_str(),
        }
    }

    pub fn provider(&self) -> &ProviderId {
        match self {
            AnyModel::Chat(m) => &m.provider,
            AnyModel::Image(m) => &m.provider,
            AnyModel::Classifier(m) => &m.provider,
        }
    }

    pub fn api(&self) -> &ApiId {
        match self {
            AnyModel::Chat(m) => &m.api,
            AnyModel::Image(m) => &m.api,
            AnyModel::Classifier(m) => &m.api,
        }
    }

    /// pi `isModelType(model, "chat")` (model-operations.ts:22-24).
    pub fn as_chat(&self) -> Option<&Model> {
        match self {
            AnyModel::Chat(m) => Some(m),
            AnyModel::Image(_) | AnyModel::Classifier(_) => None,
        }
    }

    /// pi `isModelType(model, "image")`.
    pub fn as_image(&self) -> Option<&ImageModel> {
        match self {
            AnyModel::Image(m) => Some(m),
            AnyModel::Chat(_) | AnyModel::Classifier(_) => None,
        }
    }

    /// pi `isModelType(model, "classifier")`.
    pub fn as_classifier(&self) -> Option<&ClassifierModel> {
        match self {
            AnyModel::Classifier(m) => Some(m),
            AnyModel::Chat(_) | AnyModel::Image(_) => None,
        }
    }

    pub fn into_chat(self) -> Option<Model> {
        match self {
            AnyModel::Chat(m) => Some(m),
            AnyModel::Image(_) | AnyModel::Classifier(_) => None,
        }
    }

    pub fn into_image(self) -> Option<ImageModel> {
        match self {
            AnyModel::Image(m) => Some(m),
            AnyModel::Chat(_) | AnyModel::Classifier(_) => None,
        }
    }

    pub fn into_classifier(self) -> Option<ClassifierModel> {
        match self {
            AnyModel::Classifier(m) => Some(m),
            AnyModel::Chat(_) | AnyModel::Image(_) => None,
        }
    }
}

impl From<Model> for AnyModel {
    fn from(model: Model) -> Self {
        AnyModel::Chat(model)
    }
}

impl From<ImageModel> for AnyModel {
    fn from(model: ImageModel) -> Self {
        AnyModel::Image(model)
    }
}

impl From<ClassifierModel> for AnyModel {
    fn from(model: ClassifierModel) -> Self {
        AnyModel::Classifier(model)
    }
}

impl<'de> serde::Deserialize<'de> for AnyModel {
    /// Dispatches on `type`; an absent `type` is a chat model (pi `getModelType`,
    /// model-operations.ts:17-19). A type this build does not know is an error here; the store
    /// layer drops such entries instead (pi `withKnownModelTypes`, models.ts:121-127).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            None | Some("chat") => serde_json::from_value(value)
                .map(AnyModel::Chat)
                .map_err(D::Error::custom),
            Some("image") => serde_json::from_value(value)
                .map(AnyModel::Image)
                .map_err(D::Error::custom),
            Some("classifier") => serde_json::from_value(value)
                .map(AnyModel::Classifier)
                .map_err(D::Error::custom),
            Some(other) => Err(D::Error::custom(format!("unknown model type: {other}"))),
        }
    }
}

// ----------------------------------------------------------------------------- ordered objects --

/// A string-keyed map ordered the way a JS object orders its own string keys, serialized as a JSON
/// object.
///
/// The order is behaviour, not decoration: a choice question's answer labels (`A`, `B`, ...) are
/// assigned to its `criteria` keys in object order, and the questions of one request are asked in
/// object order (pi `Object.keys` / `Object.entries` over `Record<string, _>`,
/// `llama-cpp-classify.ts:105-110`, `:445`). That order is the engine's
/// (`OrdinaryOwnPropertyKeys`, ECMA-262 10.1.11.1): keys that are canonical array indices
/// (`"0"`, `"1"`, ... up to 2^32 - 2, no leading zeros) come first in ascending numeric order, then
/// every other key in insertion order. [`OrderedMap::insert`] reproduces that, so
/// `{"b": .., "1": ..}` iterates as `1`, `b`, exactly as pi's object does. Inserting an existing key
/// replaces its value in place, as assigning to an existing JS object key does.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

/// The array index a JS engine would read `key` as (ECMA-262 "array index": a canonical integer
/// string below 2^32 - 1), or `None` for an ordinary string key.
pub(crate) fn js_array_index(key: &str) -> Option<u32> {
    let canonical = !key.is_empty()
        && key.bytes().all(|b| b.is_ascii_digit())
        && (key.len() == 1 || !key.starts_with('0'));
    if !canonical {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace; a replaced key keeps its position. Returns the previous value. A new
    /// array-index key goes into its ascending place ahead of every other key; any other new key
    /// goes last (see the type's docs).
    pub fn insert(&mut self, key: impl Into<String>, value: V) -> Option<V> {
        let key = key.into();
        if let Some((_, slot)) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            return Some(std::mem::replace(slot, value));
        }
        match js_array_index(&key) {
            Some(index) => {
                let at = self
                    .entries
                    .iter()
                    .position(|(k, _)| js_array_index(k).is_none_or(|other| other > index))
                    .unwrap_or(self.entries.len());
                self.entries.insert(at, (key, value));
            }
            None => self.entries.push((key, value)),
        }
        None
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in JS own-key order (see the type's docs).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }
}

impl<K: Into<String>, V> FromIterator<(K, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = OrderedMap::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl<V: serde::Serialize> serde::Serialize for OrderedMap<V> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (key, value) in &self.entries {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, V: serde::Deserialize<'de>> serde::Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<V>(std::marker::PhantomData<V>);
        impl<'de, V: serde::Deserialize<'de>> serde::de::Visitor<'de> for MapVisitor<V> {
            type Value = OrderedMap<V>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut map = OrderedMap::new();
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    map.insert(key, value);
                }
                Ok(map)
            }
        }
        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}

// ------------------------------------------------------------------------------------- request --

/// The two answers of a bool question and what each means (pi
/// `ClassifierBoolQuestion.criteria: { true: string; false: string }`, types.ts:648).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoolCriteria {
    #[serde(rename = "true")]
    pub when_true: String,
    #[serde(rename = "false")]
    pub when_false: String,
}

/// One question about the state (pi `ClassifierQuestion`, types.ts:633-651).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClassifierQuestion {
    /// Pick one of several options. `criteria` maps each option key to its description, in the
    /// order the options are presented (types.ts:633-637).
    Choice {
        instructions: String,
        criteria: OrderedMap<String>,
    },
    /// Rate on an ordered scale; `criteria[i]` describes level `i` (types.ts:639-643).
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
    /// Yes or no (types.ts:645-649).
    Bool {
        instructions: String,
        criteria: BoolCriteria,
    },
}

impl ClassifierQuestion {
    pub fn instructions(&self) -> &str {
        match self {
            ClassifierQuestion::Choice { instructions, .. }
            | ClassifierQuestion::Score { instructions, .. }
            | ClassifierQuestion::Bool { instructions, .. } => instructions,
        }
    }
}

/// A classification request: the state to judge, optional images, and the questions to ask about
/// it (pi `ClassifierContext`, `types.ts:682-690` @f1b2e77f5). The questions are answered in map
/// order.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassifierContext {
    /// pi `JsonObject`; key order is preserved (the workspace enables `serde_json/preserve_order`).
    pub state: serde_json::Map<String, serde_json::Value>,
    /// pi `ClassifierContext.images?: ImageContent[]` (`packages/ai/src/types.ts:682-690`
    /// @f1b2e77f5): "Images judged together with `state`. Only models whose `input` includes
    /// `"image"` accept them; other models return an error result." cyrup has no standalone
    /// `ImageContent`, so this carries [`Content::Image`] blocks (the same `{ type: "image",
    /// data, mimeType }` wire shape); [`assert_classifier_input_supported`] is the model check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<Content>>,
    pub questions: OrderedMap<ClassifierQuestion>,
}

impl ClassifierContext {
    /// pi `context.images?.length` — true when the request carries at least one image.
    pub fn has_images(&self) -> bool {
        self.images
            .as_ref()
            .is_some_and(|images| !images.is_empty())
    }
}

/// Rejects classifier images for models whose catalog entry does not accept image input (pi
/// `assertClassifierInputSupported`, `packages/ai/src/utils/model-operations.ts:46-53` @f1b2e77f5):
///
/// ```ts
/// if (context.images?.length && !model.input.includes("image")) {
///     throw new ModelsError("provider", `Model ${model.provider}/${model.id} does not accept image input`);
/// }
/// ```
///
/// The `Err` string is the error result's message.
pub fn assert_classifier_input_supported(
    model: &ClassifierModel,
    context: &ClassifierContext,
) -> Result<(), String> {
    if context.has_images() && !model.input.contains(&Modality::Image) {
        return Err(format!(
            "Model {}/{} does not accept image input",
            model.provider, model.id
        ));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------- options --

/// Inspect or replace a classifier request payload before sending (pi
/// `ProviderRequestOptions.onPayload`, types.ts:153). Returning `None` keeps the payload
/// unchanged. Async for the same reason [`crate::stream::OnPayload`] is: the extension producer
/// dispatches into wasm.
pub type ClassifierOnPayload = Arc<
    dyn Fn(
            serde_json::Value,
            ClassifierModel,
        ) -> futures::future::BoxFuture<'static, Option<serde_json::Value>>
        + Send
        + Sync,
>;

/// Invoked after an HTTP response is received (pi `ProviderRequestOptions.onResponse`,
/// types.ts:157).
pub type ClassifierOnResponse = Arc<
    dyn Fn(ProviderResponse, ClassifierModel) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// Value of [`ClassifierOptions::temperature`] when the caller sets none
/// (`options?.temperature ?? 1`, `llama-cpp-classify.ts:437`).
pub const DEFAULT_TEMPERATURE: f64 = 1.0;

/// Value of [`ClassifierOptions::max_retries`] when the caller sets none
/// (`options?.maxRetries ?? 2`, `llama-cpp-classify.ts:265`).
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// Per-request options for `classify` (pi `ClassifierOptions`, types.ts:324-331, which extends
/// `ProviderRequestOptions`, :132-185).
///
/// pi's `fetch` (types.ts:142) and `telemetryContext` (:135) are not carried; cyrup's other
/// option structs ([`crate::StreamOptions`], [`crate::ImagesOptions`]) do not carry them either.
#[derive(Clone)]
pub struct ClassifierOptions {
    /// Cancellation (pi `signal?: AbortSignal`).
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Provider-scoped environment values (pi `env`, types.ts:148).
    pub env: Option<ProviderEnv>,
    /// Per-request header overlay; a `None` value suppresses a default header (pi `headers`,
    /// types.ts:166).
    pub headers: Option<HeaderMap>,
    /// HTTP request timeout in milliseconds (pi `timeoutMs`, types.ts:171).
    pub timeout_ms: Option<u64>,
    /// Maximum retry attempts (pi `maxRetries`, types.ts:176). Defaults to
    /// [`DEFAULT_MAX_RETRIES`].
    pub max_retries: u32,
    /// Cap on a server-requested retry delay (pi `maxRetryDelayMs`, types.ts:184).
    pub max_retry_delay_ms: Option<u64>,
    /// Divides the answer logits by this value before they are normalized into probabilities.
    /// Values above 1 soften the distribution, values below 1 sharpen it; it must be positive.
    /// APIs that cannot apply it ignore it (pi `temperature`, types.ts:325-330). Defaults to
    /// [`DEFAULT_TEMPERATURE`].
    pub temperature: f64,
    pub on_payload: Option<ClassifierOnPayload>,
    pub on_response: Option<ClassifierOnResponse>,
    /// `ModelsRequestTransforms.transformHeaders` (pi `models.ts:106-109`, mixed into
    /// `ModelsClassifierOptions` at :116): transforms the fully assembled model/auth/request
    /// headers before provider dispatch. Applied by [`crate::collection::Models::classify`]
    /// and stripped from what the provider sees, exactly as pi's `applyAuth` does (:861, :864).
    pub transform_headers: Option<TransformHeadersFn>,
}

impl Default for ClassifierOptions {
    fn default() -> Self {
        Self {
            cancel: None,
            api_key: None,
            env: None,
            headers: None,
            timeout_ms: None,
            max_retries: DEFAULT_MAX_RETRIES,
            max_retry_delay_ms: None,
            temperature: DEFAULT_TEMPERATURE,
            on_payload: None,
            on_response: None,
            transform_headers: None,
        }
    }
}

impl ClassifierOptions {
    /// pi `options?.signal?.aborted`.
    pub fn is_aborted(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }

    /// Run the `on_payload` hook over an outbound body and adopt any replacement wholesale
    /// (`const transformed = await options?.onPayload?.(payload, model); if (transformed !==
    /// undefined) payload = transformed`, `llama-cpp-classify.ts:231-232`).
    pub async fn apply_on_payload(
        &self,
        model: &ClassifierModel,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        if let Some(hook) = &self.on_payload
            && let Some(replaced) = hook(payload.clone(), model.clone()).await
        {
            return replaced;
        }
        payload
    }

    /// Run the `on_response` hook (`await options?.onResponse?.(...)`, `llama-cpp-classify.ts:268`).
    pub async fn emit_on_response(&self, model: &ClassifierModel, response: ProviderResponse) {
        if let Some(hook) = &self.on_response {
            hook(response, model.clone()).await;
        }
    }
}

// ------------------------------------------------------------------------------------- result --

/// The terminal reason of a classification (pi `ClassifierStopReason`, types.ts:677).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassifierStopReason {
    Stop,
    Error,
    Aborted,
}

/// One question's answer (pi `ClassifierAnswer`, types.ts:658-676).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClassifierAnswer {
    /// The most likely option, every option's probability, and how peaked the distribution is.
    Choice {
        choice: String,
        probabilities: OrderedMap<f64>,
        confidence: f64,
    },
    /// The expected level (probability-weighted) and the confidence.
    Score { score: f64, confidence: f64 },
    /// The probability of "yes".
    Bool { probability: f64 },
}

/// The result of a classification (pi `ClassifierResult`, types.ts:679-688).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierResult {
    pub api: ApiId,
    pub provider: ProviderId,
    pub model: String,
    /// Answers by question id; empty unless [`ClassifierResult::stop_reason`] is `Stop`.
    pub answers: OrderedMap<ClassifierAnswer>,
    /// Token usage and its cost at the model's catalog price, when the service reports token
    /// counts (types.ts:685).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<Usage>,
    pub stop_reason: ClassifierStopReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
}

impl ClassifierResult {
    /// A fresh successful-by-default envelope: the `output` an api fills in
    /// (`llama-cpp-classify.ts:426-433`).
    pub fn new(model: &ClassifierModel) -> Self {
        Self {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            answers: OrderedMap::new(),
            usage: None,
            stop_reason: ClassifierStopReason::Stop,
            error_message: None,
            timestamp: crate::images::now_ms(),
        }
    }

    /// The terminal error envelope (pi `classifierErrorResult`, model-operations.ts:56-70):
    /// no answers, `error` or `aborted`, and the message. Every failure of `classify` is
    /// delivered this way; it never rejects.
    pub fn errored(model: &ClassifierModel, message: impl Into<String>, aborted: bool) -> Self {
        Self {
            stop_reason: if aborted {
                ClassifierStopReason::Aborted
            } else {
                ClassifierStopReason::Error
            },
            error_message: Some(message.into()),
            ..Self::new(model)
        }
    }
}

// ------------------------------------------------------------------------------ implementations --

/// The uniform contract implemented by classifier api modules (pi `ProviderClassifier`,
/// types.ts:315-322).
///
/// **Never returns an error**: every failure, including an unsupported `model.api`, bad options,
/// transport errors and cancellation, is encoded in the returned [`ClassifierResult`]
/// (`stop_reason` `error` / `aborted` plus `error_message`, `answers` empty), exactly as pi's
/// `classify` resolves rather than rejects (`llama-cpp-classify.ts:452-457`).
#[async_trait::async_trait]
pub trait ProviderClassifier: Send + Sync {
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult;
}

/// Classifier implementations keyed by `model.api` (pi `CreateProviderOptions.classifiers`,
/// models.ts:1022-1023, dispatched at :1161-1171), optionally layered over a fallback
/// classifier (pi `composeProvider`'s `extensionClassifiers` over `base.classify`,
/// `provider-composer.ts:652-664`).
///
/// It is itself a [`ProviderClassifier`], so a provider's `classify` is one delegating call. A
/// model whose api has no entry and no fallback answers with an error result naming the provider
/// and the api (`Provider ${id} has no classifier implementation for "${model.api}"`,
/// models.ts:1167; provider-composer.ts:659).
#[derive(Clone, Default)]
pub struct ClassifierApiRegistry {
    apis: BTreeMap<String, Arc<dyn ProviderClassifier>>,
    fallback: Option<Arc<dyn ProviderClassifier>>,
}

impl ClassifierApiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the implementation of `api`. A [`KnownClassifierApi`], a `&str`
    /// and a `String` all convert.
    pub fn register(
        &mut self,
        api: impl Into<ApiId>,
        classifier: Arc<dyn ProviderClassifier>,
    ) -> &mut Self {
        self.apis
            .insert(api.into().as_str().to_string(), classifier);
        self
    }

    /// Consulted when no registered api matches (pi `if (classify) return classify(...)`,
    /// provider-composer.ts:655).
    #[must_use]
    pub fn with_fallback(mut self, fallback: Arc<dyn ProviderClassifier>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// The implementation registered for exactly `api`.
    pub fn get(&self, api: &str) -> Option<Arc<dyn ProviderClassifier>> {
        self.apis.get(api).cloned()
    }

    pub fn contains(&self, api: &str) -> bool {
        self.apis.contains_key(api)
    }

    /// The registered api ids, sorted.
    pub fn apis(&self) -> Vec<&str> {
        self.apis.keys().map(String::as_str).collect()
    }

    /// `true` when nothing is registered and there is no fallback (pi's `createProvider` rejects
    /// an empty `classifiers` map, models.ts:1045).
    pub fn is_empty(&self) -> bool {
        self.apis.is_empty() && self.fallback.is_none()
    }
}

#[async_trait::async_trait]
impl ProviderClassifier for ClassifierApiRegistry {
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        if let Some(implementation) = self.apis.get(model.api.as_str()) {
            return implementation.classify(model, context, options).await;
        }
        if let Some(fallback) = &self.fallback {
            return fallback.classify(model, context, options).await;
        }
        ClassifierResult::errored(
            model,
            format!(
                "Provider {} has no classifier implementation for \"{}\"",
                model.provider, model.api
            ),
            false,
        )
    }
}
