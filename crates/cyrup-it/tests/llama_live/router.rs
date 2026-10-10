//! The opt-in gate and the real `llama-server` router the tests start.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The binary to start (`LLAMA_SERVER_BIN`).
pub const BIN_VAR: &str = "LLAMA_SERVER_BIN";
/// The directory holding the two test models (`CYRUP_LLAMA_MODELS_DIR`).
pub const MODELS_VAR: &str = "CYRUP_LLAMA_MODELS_DIR";
/// The text model: `output_modalities: ["text"]`.
pub const CHAT_MODEL: &str = "stories260K";
/// The decision model: `output_modalities: ["decisions"]`.
pub const DECISION_MODEL: &str = "tinylaya-for-testing-Q8_0";
/// The key the router is started with (`--api-key`), so every request also proves auth.
pub const API_KEY: &str = "sk-cyrup-llama-live";

/// What the two variables name, once both are set and point at something real.
pub struct Setup {
    bin: PathBuf,
    models_dir: PathBuf,
}

/// The gate. `None` (after writing a SKIPPED line straight to stderr, which libtest does not
/// capture) when NEITHER variable is set; a panic when only one is, or when what they name is
/// missing, so a half-configured run fails instead of passing quietly.
pub fn setup(test: &str) -> Option<Setup> {
    let bin = std::env::var_os(BIN_VAR).filter(|value| !value.is_empty());
    let models_dir = std::env::var_os(MODELS_VAR).filter(|value| !value.is_empty());
    match (bin, models_dir) {
        (None, None) => {
            // `eprintln!` would go to libtest's capture buffer and vanish on a pass; a direct
            // write to the stderr handle does not.
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED (llama_live): {test}: set {BIN_VAR} to a llama-server built from \
                 llama.cpp b11436 and {MODELS_VAR} to a directory holding {CHAT_MODEL}.gguf and \
                 {DECISION_MODEL}.gguf to run it (see tests/llama_live/main.rs)"
            );
            None
        }
        (Some(bin), Some(models_dir)) => {
            let setup = Setup {
                bin: PathBuf::from(bin),
                models_dir: PathBuf::from(models_dir),
            };
            assert!(
                setup.bin.is_file(),
                "{BIN_VAR}={} is not a file",
                setup.bin.display()
            );
            for model in [CHAT_MODEL, DECISION_MODEL] {
                let file = setup.models_dir.join(format!("{model}.gguf"));
                assert!(
                    file.is_file(),
                    "{MODELS_VAR} holds no {} (both test models are required)",
                    file.display()
                );
            }
            Some(setup)
        }
        (bin, models_dir) => panic!(
            "llama_live is half-configured: {BIN_VAR} is {}, {MODELS_VAR} is {}; set both or neither",
            if bin.is_some() { "set" } else { "unset" },
            if models_dir.is_some() { "set" } else { "unset" },
        ),
    }
}

/// A running `llama-server --models-dir` router, killed on drop. Its children (one per loaded
/// model) watch their stdin and exit when the router goes (`server-models.cpp:1834-1853` @b11436).
pub struct Router {
    child: Child,
    url: String,
    /// The router's own log, for the panic message when it does not come up.
    log: PathBuf,
    _scratch: tempfile::TempDir,
}

impl Drop for Router {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // A failing test gets the end of the router's log (its children log there too), which is
        // the only record of what the real server saw.
        if std::thread::panicking() {
            let log = std::fs::read_to_string(&self.log).unwrap_or_default();
            let lines: Vec<&str> = log.lines().collect();
            let tail = lines
                .get(lines.len().saturating_sub(80)..)
                .unwrap_or_default();
            let _ = writeln!(
                std::io::stderr(),
                "---- the last {} lines of the llama-server log ----\n{}",
                tail.len(),
                tail.join("\n")
            );
        }
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

impl Router {
    /// Start the router on a free loopback port and wait until `GET /models` answers.
    pub async fn start(setup: &Setup) -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let log = scratch.path().join("llama-server.log");
        let port = free_port();
        let mut command = Command::new(&setup.bin);
        command
            .arg("--models-dir")
            .arg(&setup.models_dir)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-t", "1", "--api-key", API_KEY])
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap());
        crate::support::env::scrub(&mut command);
        let child = command.spawn().unwrap();
        let router = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
            log,
            _scratch: scratch,
        };
        router.wait_ready().await;
        router
    }

    async fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok((200, _)) = self.get("/models").await {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "llama-server did not answer GET /models within 60 s; its log:\n{}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `http://127.0.0.1:<port>`, the server root.
    pub fn url(&self) -> &str {
        &self.url
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<(u16, String), reqwest::Error> {
        let response = request.send().await?;
        let status = response.status().as_u16();
        Ok((status, response.text().await?))
    }

    async fn get(&self, path: &str) -> Result<(u16, String), reqwest::Error> {
        self.send(
            Self::http()
                .get(format!("{}{path}", self.url))
                .bearer_auth(API_KEY),
        )
        .await
    }

    /// The status and the raw body of an authenticated `GET`.
    pub async fn raw_get(&self, path: &str) -> (u16, String) {
        self.get(path).await.unwrap()
    }

    /// The status and the raw body of a `GET` with NO key.
    pub async fn raw_get_unauthenticated(&self, path: &str) -> (u16, String) {
        self.send(Self::http().get(format!("{}{path}", self.url)))
            .await
            .unwrap()
    }

    /// The status and the raw body of an authenticated JSON `POST`.
    pub async fn raw_post(&self, path: &str, body: &Value) -> (u16, String) {
        self.send(
            Self::http()
                .post(format!("{}{path}", self.url))
                .bearer_auth(API_KEY)
                .json(body),
        )
        .await
        .unwrap()
    }

    /// An authenticated JSON `POST` whose answer must be a 200 JSON body.
    pub async fn post_json(&self, path: &str, body: &Value) -> Value {
        let (status, text) = self.raw_post(path, body).await;
        assert_eq!(status, 200, "POST {path}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// An authenticated `GET` whose answer must be a 200 JSON body.
    pub async fn get_json(&self, path: &str) -> Value {
        let (status, text) = self.raw_get(path).await;
        assert_eq!(status, 200, "GET {path}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// Open `GET /models/sse` and collect its raw text in the background until the handle is
    /// stopped.
    pub fn tail_sse(&self) -> SseTail {
        let request = Self::http()
            .get(format!("{}/models/sse", self.url))
            .bearer_auth(API_KEY);
        let text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = text.clone();
        let task = tokio::spawn(async move {
            use futures::StreamExt as _;
            let Ok(response) = request.send().await else {
                return;
            };
            let mut stream = response.bytes_stream();
            while let Some(Ok(chunk)) = stream.next().await {
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk));
            }
        });
        SseTail { text, task }
    }
}

/// A background reader of `GET /models/sse`.
pub struct SseTail {
    text: std::sync::Arc<std::sync::Mutex<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl SseTail {
    /// Everything read so far.
    pub fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }

    /// Stop reading and return everything read.
    pub fn stop(self) -> String {
        self.task.abort();
        self.text()
    }
}
