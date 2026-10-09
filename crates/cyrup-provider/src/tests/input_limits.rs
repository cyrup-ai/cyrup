//! `PROV-134` — `inputLimits` on the shared base model shape: the type, the three declaration
//! surfaces, the serde round-trip through both hand-written wire mirrors, and the generator-time
//! stamp.
//!
//! Upstream, read at **v1.0.4** through git objects (`git -C tmp/pi show v1.0.4:<path>`):
//!
//! | what | where |
//! |---|---|
//! | the three interfaces, and `inputLimits` on `BaseModel` between `input` and `cost` | `packages/ai/src/types.ts:1075-1096`, `:1105` |
//! | `applyImageInputMetadata` + `DEFAULT_IMAGE_RESIZE` | `packages/ai/scripts/generate-models.ts:994-1020`, `:424-430` |
//! | its two call sites (chat+classifier, then openrouter images) | `generate-models.ts:3487`, `:3505` |
//! | the oracle for the stamp's numbers | `packages/ai/test/providers.test.ts:111-136` |
//! | `maxRequestBytes` / `maxPerMessage` / `maxPerRequest` are DESCRIPTIVE | `packages/coding-agent/docs/models.md:89` |
//!
//! # Why some of these rows are text-only on purpose
//!
//! The stamp fills `images.resize` on EVERY image-capable row, which means on an image-capable row
//! a correct profile proves nothing about serde — the stamp would synthesise the same four numbers
//! from an empty row. So the serde half is tested two ways the stamp cannot fake: on a TEXT-ONLY
//! row (where the stamp returns early, so every value present can only have been deserialized),
//! and with NON-DEFAULT values the stamp has no way to invent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::catalog::DEFAULT_IMAGE_RESIZE;
use crate::classifier::{AnyModel, ClassifierModel, ImageModel};
use crate::model::{Modality, Model, ModelImageInputLimits, ModelImageResizeOptions};

/// A chat catalog row as JSON, so every test goes through the real `load_catalog` parse-and-stamp
/// path rather than a hand-built struct (the same helper shape as `tests::prompt_cache`).
fn chat_row(provider: &str, input: &str, context_window: u64, extra: &str) -> String {
    format!(
        r#"[{{"id":"m","name":"M","api":"anthropic-messages","provider":"{provider}",
             "baseUrl":"https://x","reasoning":false,"input":{input},
             "cost":{{"input":1.0,"output":1.0,"cacheRead":0.1,"cacheWrite":1.25}},
             "contextWindow":{context_window},"maxTokens":8192{extra}}}]"#
    )
}

fn one_chat(json: &str) -> Model {
    crate::catalog::load_catalog(json)
        .expect("catalog parses")
        .pop()
        .expect("one row")
}

fn resize(model: &Model) -> ModelImageResizeOptions {
    model
        .input_limits
        .as_ref()
        .and_then(|l| l.images.as_ref())
        .and_then(|i| i.resize.clone())
        .expect("an image-capable row resolves a resize profile")
}

// ------------------------------------------------------------------- serde, where the stamp is --
// ------------------------------------------------------------------- provably not the source ----

/// The serde half, proved on a row the stamp cannot touch: `input: ["text"]` takes
/// `applyImageInputMetadata`'s early return (`generate-models.ts:995`), so every one of these
/// values can ONLY have come from the deserializer. All four are deliberately non-default.
#[test]
fn a_text_only_rows_declared_limits_survive_the_parse_untouched() {
    let m = one_chat(&chat_row(
        "openrouter",
        r#"["text"]"#,
        128_000,
        r#","inputLimits":{"maxRequestBytes":4242,
             "images":{"maxPerMessage":3,"maxPerRequest":7,
                       "resize":{"maxWidth":1234,"maxHeight":567,"maxBytes":89,"jpegQuality":42}}}"#,
    ));
    let limits = m.input_limits.expect("declared limits are not dropped");
    assert_eq!(limits.max_request_bytes, Some(4242));
    assert_eq!(
        limits.images,
        Some(ModelImageInputLimits {
            resize: Some(ModelImageResizeOptions {
                max_width: Some(1234),
                max_height: Some(567),
                max_bytes: Some(89),
                jpeg_quality: Some(42),
            }),
            max_per_message: Some(3),
            max_per_request: Some(7),
        })
    );
}

/// The 1011 rows this field recovers. Measured at HEAD over the 39 embedded catalogs: 1519 rows,
/// 1065 image-capable, 1011 of them carrying `inputLimits` — and NOT ONE text-only row carrying
/// it, which is exactly `applyImageInputMetadata`'s `input.includes("image")` guard showing
/// through the data. Before this field existed serde dropped every one of them on parse and the
/// models store re-serialized without them.
///
/// Asserted on the real embedded catalog rather than a synthetic row, and on the invariant rather
/// than on a model id, so a catalog refresh cannot make it stale.
#[test]
fn every_image_capable_row_of_the_embedded_catalogs_resolves_a_resize_profile() {
    let (mut image_capable, mut text_only) = (0usize, 0usize);
    for model in crate::catalog::builtin_catalog() {
        if model.supports_image_input() {
            image_capable += 1;
            let got = model
                .input_limits
                .as_ref()
                .and_then(|l| l.images.as_ref())
                .and_then(|i| i.resize.clone());
            assert_eq!(
                got,
                Some(DEFAULT_IMAGE_RESIZE),
                "{}/{} resolves no resize profile — does its catalog bypass load_catalog?",
                model.provider.as_str(),
                model.id.as_str()
            );
        } else {
            text_only += 1;
            assert!(
                model.input_limits.is_none(),
                "{} is text-only and must carry no inputLimits (upstream's guard)",
                model.id.as_str()
            );
        }
    }
    assert!(
        image_capable > 1000 && text_only > 400,
        "catalog census moved a lot: {image_capable} image-capable / {text_only} text-only"
    );
}

/// The live pi.dev overlay body, through the real `parse_catalog` path. 270 of its 400 chat rows
/// and 57 of its 59 image rows carry `inputLimits`; the 2 image rows that do not are
/// `input: ["text"]`, and all 10 classifier rows are text-only. Before this field the overlay
/// destroyed the profile on the way in AND again on the way to `models-store.json`.
#[test]
fn the_live_overlay_body_keeps_its_profile_on_chat_and_image_rows_alike() {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pi-dev-openrouter-all-types.json")).unwrap();
    let rows = crate::remote_catalog::parse_catalog("openrouter", &body).unwrap();

    let (mut chat, mut image, mut classifier) = (0usize, 0usize, 0usize);
    for row in &rows {
        match row {
            AnyModel::Chat(m) => {
                if m.input.contains(&Modality::Image) {
                    chat += 1;
                    assert_eq!(resize(m), DEFAULT_IMAGE_RESIZE, "{}", m.id.as_str());
                }
            }
            AnyModel::Image(m) => {
                if m.input.contains(&Modality::Image) {
                    image += 1;
                    let got = m
                        .input_limits
                        .as_ref()
                        .and_then(|l| l.images.as_ref())
                        .and_then(|i| i.resize.clone());
                    assert_eq!(got, Some(DEFAULT_IMAGE_RESIZE), "{}", m.id.as_str());
                }
            }
            AnyModel::Classifier(m) => {
                classifier += 1;
                assert!(m.input_limits.is_none(), "{}", m.id.as_str());
            }
        }
    }
    assert_eq!(
        (chat, image, classifier),
        (270, 57, 10),
        "the fixture's own census (captured 2026-10-05)"
    );
}

/// The remote overlay gets the stamp too, on all THREE variants — the SEAM-131 precedent in
/// `parse_catalog`, for the same reason: `merge_models` replaces a baseline entry wholesale by id,
/// so a refreshed row served without `inputLimits` would land with no profile at all and the
/// resizer would silently see nothing where the embedded row had something.
///
/// It is a FLOOR, not a mechanism: today's pi.dev body is already stamped on every image-capable
/// row, so deleting this call leaves
/// [`the_live_overlay_body_keeps_its_profile_on_chat_and_image_rows_alike`] green. This test is
/// what makes it load-bearing — the body here deliberately carries NO `inputLimits`, which the
/// live one never does, so only the stamp can produce the profile.
#[test]
fn an_overlay_row_served_without_the_field_is_stamped_on_all_three_variants() {
    let body: serde_json::Value = serde_json::from_str(
        r#"[
        {"id":"c","name":"C","api":"openai-completions","provider":"x","baseUrl":"https://x",
         "reasoning":false,"input":["text","image"],
         "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0},
         "contextWindow":128000,"maxTokens":4096},
        {"type":"image","id":"i","name":"I","api":"openrouter-images","provider":"x",
         "baseUrl":"https://x","input":["text","image"],"output":["image"],
         "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
        {"type":"classifier","id":"k","name":"K","api":"llama-cpp-classify","provider":"x",
         "baseUrl":"https://x","input":["text","image"],
         "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0},
         "contextWindow":4096}
    ]"#,
    )
    .unwrap();
    let rows = crate::remote_catalog::parse_catalog("x", &body).unwrap();
    assert_eq!(rows.len(), 3, "one row of each type");
    for row in &rows {
        let got = match row {
            AnyModel::Chat(m) => m.input_limits.clone(),
            AnyModel::Image(m) => m.input_limits.clone(),
            AnyModel::Classifier(m) => m.input_limits.clone(),
        };
        assert_eq!(
            got.and_then(|l| l.images).and_then(|i| i.resize),
            Some(DEFAULT_IMAGE_RESIZE),
            "an overlay row of this type was not stamped"
        );
    }
}

/// `ImageModel` and `ClassifierModel` read through hand-written `*Wire` mirrors that copy field by
/// field. A field added to the public struct but forgotten in the mirror COMPILES, serializes
/// correctly, and silently reads `None` on every row — the very bug this row fixes, reintroduced
/// one layer down. Non-default values on a text-only row, so neither the stamp nor a default can
/// stand in for the mirror.
#[test]
fn the_image_and_classifier_wire_mirrors_both_copy_the_field() {
    let limits = r#""inputLimits":{"maxRequestBytes":11,
         "images":{"maxPerMessage":2,"maxPerRequest":3,
                   "resize":{"maxWidth":17,"maxHeight":19,"maxBytes":23,"jpegQuality":29}}}"#;
    let expected = Some(ModelImageInputLimits {
        resize: Some(ModelImageResizeOptions {
            max_width: Some(17),
            max_height: Some(19),
            max_bytes: Some(23),
            jpeg_quality: Some(29),
        }),
        max_per_message: Some(2),
        max_per_request: Some(3),
    });

    let image: ImageModel = serde_json::from_str(&format!(
        r#"{{"type":"image","id":"i","name":"I","api":"openrouter-images",
             "provider":"openrouter","baseUrl":"https://x","input":["text"],"output":["image"],
             "cost":{{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},{limits}}}"#
    ))
    .expect("image row parses");
    assert_eq!(
        image.input_limits.as_ref().and_then(|l| l.images.clone()),
        expected,
        "ImageModelWire dropped the field"
    );
    assert_eq!(
        image
            .input_limits
            .as_ref()
            .and_then(|l| l.max_request_bytes),
        Some(11)
    );

    let classifier: ClassifierModel = serde_json::from_str(&format!(
        r#"{{"type":"classifier","id":"c","name":"C","api":"llama-cpp-classify",
             "provider":"llama-cpp","baseUrl":"https://x","input":["text"],
             "cost":{{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
             "contextWindow":4096,{limits}}}"#
    ))
    .expect("classifier row parses");
    assert_eq!(
        classifier
            .input_limits
            .as_ref()
            .and_then(|l| l.images.clone()),
        expected,
        "ClassifierModelWire dropped the field"
    );
}

/// The two `to_auth_model` shims CARRY the field rather than nulling it. `prompt_cache: None` is
/// right there only because an image/classifier row has no source for a chat-only member; this
/// one is on `BaseModel`, so it does.
#[test]
fn the_auth_shims_carry_the_field_rather_than_dropping_it() {
    let images = crate::providers::openrouter::openrouter_image_models();
    let row = images
        .iter()
        .find(|m| m.input.contains(&Modality::Image))
        .expect("the frozen image catalog has image-capable rows");
    assert_eq!(
        row.to_auth_model().input_limits,
        row.input_limits,
        "ImageModel::to_auth_model must not drop inputLimits"
    );

    let classifier = crate::tests::classifier_support::classifier_model("p", "c", "a");
    let mut with = classifier.clone();
    with.input_limits = Some(crate::model::ModelInputLimits {
        max_request_bytes: Some(99),
        images: None,
    });
    assert_eq!(with.to_auth_model().input_limits, with.input_limits);
}

// --------------------------------------------------------------- the generator-time stamp ------

/// Upstream's own assertion (`providers.test.ts:117-124`): a direct-Anthropic row whose
/// `contextWindow` is exactly 200000 and whose type is not `image` gets `maxPerRequest: 100` and
/// `maxRequestBytes: 32 MiB`.
#[test]
fn a_200k_direct_anthropic_chat_row_gets_100_images_per_request() {
    let m = one_chat(&chat_row("anthropic", r#"["text","image"]"#, 200_000, ""));
    let limits = m.input_limits.expect("stamped");
    assert_eq!(limits.max_request_bytes, Some(33_554_432));
    assert_eq!(
        limits.images.and_then(|i| i.max_per_request),
        Some(100),
        "contextWindow === 200000 ? 100 : 600"
    );
}

/// The other side of the same branch (`providers.test.ts:120`: `claude-opus-5` resolves 600).
#[test]
fn an_anthropic_row_off_the_200k_window_gets_600() {
    let m = one_chat(&chat_row("anthropic", r#"["text","image"]"#, 1_000_000, ""));
    assert_eq!(
        m.input_limits
            .and_then(|l| l.images)
            .and_then(|i| i.max_per_request),
        Some(600)
    );
}

/// The remaining three provider branches of `generate-models.ts:997-1008`, verbatim.
#[test]
fn the_openai_google_and_bedrock_branches_are_ported_verbatim() {
    let openai = one_chat(&chat_row("openai", r#"["text","image"]"#, 400_000, ""))
        .input_limits
        .expect("stamped");
    assert_eq!(openai.max_request_bytes, Some(536_870_912));
    assert_eq!(
        openai.images.as_ref().and_then(|i| i.max_per_request),
        Some(1500)
    );
    assert_eq!(openai.images.and_then(|i| i.max_per_message), None);

    let google = one_chat(&chat_row("google", r#"["text","image"]"#, 1_000_000, ""))
        .input_limits
        .expect("stamped");
    assert_eq!(google.max_request_bytes, Some(20_971_520));
    assert_eq!(google.images.and_then(|i| i.max_per_request), Some(3600));

    let bedrock = one_chat(&chat_row(
        "amazon-bedrock",
        r#"["text","image"]"#,
        200_000,
        "",
    ))
    .input_limits
    .expect("stamped");
    assert_eq!(
        bedrock.max_request_bytes, None,
        "bedrock's branch sets no maxRequestBytes"
    );
    let images = bedrock.images.expect("images");
    assert_eq!(images.max_per_message, Some(20));
    assert_eq!(images.max_per_request, None);
}

/// Upstream's `providers.test.ts:136`: an openrouter row resolves EXACTLY
/// `{ images: { resize: DEFAULT_IMAGE_RESIZE } }` — no `maxRequestBytes`, no per-message or
/// per-request count, because the provider table's fallthrough is `undefined`.
#[test]
fn an_unknown_provider_gets_the_cache_safe_resize_default_and_nothing_else() {
    let m = one_chat(&chat_row("openrouter", r#"["text","image"]"#, 200_000, ""));
    assert_eq!(
        m.input_limits,
        Some(crate::model::ModelInputLimits {
            max_request_bytes: None,
            images: Some(ModelImageInputLimits {
                resize: Some(DEFAULT_IMAGE_RESIZE),
                max_per_message: None,
                max_per_request: None,
            }),
        })
    );
    assert_eq!(
        DEFAULT_IMAGE_RESIZE,
        ModelImageResizeOptions {
            max_width: Some(2000),
            max_height: Some(2000),
            max_bytes: Some(4_718_592),
            jpeg_quality: Some(80),
        },
        "DEFAULT_IMAGE_RESIZE, generate-models.ts:424-430; 4_718_592 == 4.5 * 1024 * 1024"
    );
}

/// `if (!model.input.includes("image")) return;` — a text-only row is left entirely alone, which
/// is what makes the field's ABSENCE meaningful.
#[test]
fn a_text_only_row_is_not_stamped_at_all() {
    let m = one_chat(&chat_row("anthropic", r#"["text"]"#, 200_000, ""));
    assert!(m.input_limits.is_none());
}

/// `resize: { ...DEFAULT_IMAGE_RESIZE, ...configuredImages?.resize }` is a PER-KEY merge: a row
/// declaring one key keeps it and gains the other three. Collapsing it to "the row's resize wins
/// whole" would leave the other three unset and the resizer would silently fall back three times.
#[test]
fn a_row_declaring_one_resize_key_keeps_it_and_gains_the_other_three() {
    let m = one_chat(&chat_row(
        "openrouter",
        r#"["text","image"]"#,
        200_000,
        r#","inputLimits":{"images":{"resize":{"jpegQuality":50}}}"#,
    ));
    assert_eq!(
        resize(&m),
        ModelImageResizeOptions {
            max_width: Some(2000),
            max_height: Some(2000),
            max_bytes: Some(4_718_592),
            jpeg_quality: Some(50),
        }
    );
}

/// `{ ...providerLimits, ...model.inputLimits }` and `{ ...providerLimits?.images,
/// ...configuredImages }`: the ROW wins over the provider table, at both levels.
#[test]
fn a_row_wins_over_the_provider_table_at_both_levels() {
    let m = one_chat(&chat_row(
        "anthropic",
        r#"["text","image"]"#,
        200_000,
        r#","inputLimits":{"maxRequestBytes":7,"images":{"maxPerRequest":9}}"#,
    ));
    let limits = m.input_limits.expect("stamped");
    assert_eq!(limits.max_request_bytes, Some(7), "row's 7 beats 32 MiB");
    let images = limits.images.expect("images");
    assert_eq!(images.max_per_request, Some(9), "row's 9 beats 100");
    assert_eq!(
        images.resize,
        Some(DEFAULT_IMAGE_RESIZE),
        "and resize is still filled beside it"
    );
}

/// The one place the stamp is NOT already a no-op. `catalog/openrouter-images.json` was frozen at
/// v0.87.1, before `inputLimits` existed on any model shape, so **0 of its 54 image-capable rows
/// carry one** — a measurement, not an estimate. Upstream stamps the image catalog with the same
/// pass (`generate-models.ts:3505`). This assertion cannot pass accidentally: the value is absent
/// from the data, so only the stamp can produce it.
#[test]
fn the_frozen_image_catalog_is_stamped_because_its_rows_carry_nothing() {
    let raw: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../providers/catalog/openrouter-images.json")).unwrap();
    assert_eq!(
        raw.iter()
            .filter(|r| r.get("inputLimits").is_some())
            .count(),
        0,
        "the premise: the frozen file declares no inputLimits at all"
    );

    let rows = crate::providers::openrouter::openrouter_image_models();
    let image_capable: Vec<&ImageModel> = rows
        .iter()
        .filter(|m| m.input.contains(&Modality::Image))
        .collect();
    assert_eq!(image_capable.len(), 54, "the file's own census");
    for m in image_capable {
        assert_eq!(
            m.input_limits
                .as_ref()
                .and_then(|l| l.images.as_ref())
                .and_then(|i| i.resize.clone()),
            Some(DEFAULT_IMAGE_RESIZE),
            "{} has no resize profile",
            m.id.as_str()
        );
    }
    // The one text-only row stays untouched, so the guard is live on this path too.
    assert!(
        rows.iter()
            .filter(|m| !m.input.contains(&Modality::Image))
            .all(|m| m.input_limits.is_none())
    );
}

/// A classifier row goes through upstream's FIRST call site, with the chat models
/// (`generate-models.ts:3487`), so `type !== "image"` is true for it and its own `contextWindow`
/// decides 100 vs 600.
#[test]
fn an_image_capable_classifier_row_is_stamped_like_a_chat_row() {
    let mut m: ClassifierModel = serde_json::from_str(
        r#"{"type":"classifier","id":"c","name":"C","api":"llama-cpp-classify",
            "provider":"anthropic","baseUrl":"https://x","input":["text","image"],
            "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0},
            "contextWindow":200000}"#,
    )
    .unwrap();
    crate::catalog::apply_image_input_metadata_to_classifier(&mut m);
    let limits = m.input_limits.expect("stamped");
    assert_eq!(limits.max_request_bytes, Some(33_554_432));
    let images = limits.images.expect("images");
    assert_eq!(images.max_per_request, Some(100));
    assert_eq!(images.resize, Some(DEFAULT_IMAGE_RESIZE));
}

/// An IMAGE row short-circuits Anthropic's `type !== "image"` to the 600 side even at a 200k-shaped
/// window, because `ImageModel` has no `contextWindow` at all (`types.ts:1097-1108`).
#[test]
fn an_anthropic_image_row_always_gets_600_because_it_has_no_context_window() {
    let mut m: ImageModel = serde_json::from_str(
        r#"{"type":"image","id":"i","name":"I","api":"openrouter-images",
            "provider":"anthropic","baseUrl":"https://x","input":["text","image"],
            "output":["image"],
            "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}}"#,
    )
    .unwrap();
    crate::catalog::apply_image_input_metadata_to_image(&mut m);
    assert_eq!(
        m.input_limits
            .and_then(|l| l.images)
            .and_then(|i| i.max_per_request),
        Some(600)
    );
}

// ------------------------------------------------------------------------ the wire shape -------

/// The field round-trips in pi's camelCase and is ELIDED when absent — the half that keeps an
/// unstamped row byte-identical to pi's output. An emitted `"inputLimits":null` is a wire
/// difference, so the negative assertion is on the substring.
#[test]
fn input_limits_round_trips_in_camel_case_and_is_elided_when_absent() {
    let stamped = one_chat(&chat_row("openrouter", r#"["text","image"]"#, 200_000, ""));
    let json = serde_json::to_string(&stamped).unwrap();
    assert!(
        json.contains(
            r#""inputLimits":{"images":{"resize":{"maxWidth":2000,"maxHeight":2000,"maxBytes":4718592,"jpegQuality":80}}}"#
        ),
        "camelCase, declared key order, and nothing emitted for the absent siblings: {json}"
    );

    let bare = one_chat(&chat_row("openrouter", r#"["text"]"#, 200_000, ""));
    let json = serde_json::to_string(&bare).unwrap();
    assert!(
        !json.contains("inputLimits"),
        "an absent field must not serialize at all: {json}"
    );

    // Re-reading what we wrote is lossless, which is what the models-store overlay depends on.
    let reparsed: Model = serde_json::from_str(&serde_json::to_string(&stamped).unwrap()).unwrap();
    assert_eq!(reparsed.input_limits, stamped.input_limits);
}

/// The same elision on the two sibling types, whose `Serialize` is derived but whose `Deserialize`
/// is not.
#[test]
fn the_sibling_types_elide_an_absent_field_too() {
    let image = crate::tests::classifier_support::image_model("p", "i", "openrouter-images");
    assert!(
        !serde_json::to_string(&image)
            .unwrap()
            .contains("inputLimits")
    );
    let classifier = crate::tests::classifier_support::classifier_model("p", "c", "a");
    assert!(
        !serde_json::to_string(&classifier)
            .unwrap()
            .contains("inputLimits")
    );
}

/// PROV-134's own Verify closes with "a row without the field still parses".
///
/// **Reported as weak on purpose.** It passes with the whole feature deleted, so it is evidence of
/// nothing on its own; it is here only as a guard against a non-`Option` field or a missing
/// `#[serde(default)]` on one of the two wire mirrors, either of which would turn every
/// pre-existing catalog row into a parse failure.
#[test]
fn a_row_with_no_input_limits_still_parses_weak_guard() {
    assert!(
        crate::catalog::load_catalog(&chat_row("openrouter", r#"["text"]"#, 200_000, "")).is_ok()
    );
    assert!(!crate::catalog::builtin_catalog().is_empty());
}
