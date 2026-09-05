use super::argv::{ExtFlagValue, ExtensionFlag};

/// Render Pi's rich `--help` body (args.ts:212-389): usage, the package/config commands, the full
/// option list, the registered-extension-flag block, examples, the environment-variable catalogue,
/// and the built-in tool names. `extension_flags` are the flags loaded extensions registered; the bin
/// passes an empty slice today (the loaded-extension flag tier is the outer extension layer,
/// ledgered), but the injection point is preserved 1:1.
///
/// SEAM-111 — the Commands block had drifted from `args.ts:226-235` in three places, and **two of
/// them understated what actually ships**:
///
/// * `update` read `Update cyrup (use --all for cyrup and extensions)`, dropping pi's model-catalog
///   clause (`args.ts:232`). `cyrup update --models` exists now (SEAM-100 landed it —
///   `subcommands.rs`'s `UpdateTargetSel::Models`), so the clause is true and is restored verbatim.
/// * `config` read `cyrup config` with no `[-l]` and no Tab hint (`args.ts:234`), yet BOTH ship:
///   `-l` is parsed at `subcommands.rs`'s config arm and Tab switches write scope in the picker. The
///   two least guessable parts of `config` were invisible from the top-level help and — until
///   SEAM-079 — from `config --help` as well.
pub fn render_help(extension_flags: &[ExtensionFlag]) -> String {
    const APP: &str = "cyrup";
    const CFG: &str = ".cyrup";
    const ENV_AGENT_DIR: &str = "CYRUP_AGENT_DIR";
    const ENV_SESSION_DIR: &str = "CYRUP_SESSION_DIR";
    let ext_block = if extension_flags.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = extension_flags
            .iter()
            .map(|f| {
                let value = if matches!(f.value, ExtFlagValue::Str(_)) {
                    " <value>"
                } else {
                    ""
                };
                format!("  --{}{}", f.name, value)
            })
            .collect();
        format!("\nExtension CLI Flags:\n{}\n", lines.join("\n"))
    };
    format!(
        "{APP} - AI coding assistant with read, bash, edit, write tools

Usage:
  {APP} [options] [@files...] [messages...]

Commands:
  {APP} install <source> [-l]     Install extension source and add to settings
  {APP} remove <source> [-l]      Remove extension source from settings
  {APP} uninstall <source> [-l]   Alias for remove
  {APP} update [source|self|pi]   Update {APP}, extensions, or model catalogs
  {APP} list                      List installed extensions from settings
  {APP} config [-l]               Open TUI to enable/disable package resources (Tab switches scope)
  {APP} auth <command>            Print credentials for external clients
  {APP} <command> --help          Show help for install/remove/uninstall/update/list/config/auth

Options:
  --provider <name>              Provider name (default: google)
  --model <pattern>              Model pattern or ID (supports \"provider/id\" and optional \":<thinking>\")
  --api-key <key>                API key (defaults to env vars)
  --system-prompt <text>         System prompt (default: coding assistant prompt)
  --append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)
  --mode <mode>                  Output mode: text (default), json, or rpc
  --json                         cyrup alias for --mode json (not a pi flag)
  --rpc                          cyrup alias for --mode rpc (not a pi flag)
  --acp                          cyrup alias for --mode acp: serve the Agent Client Protocol on stdio
  --output-format <fmt>          cyrup alias: text = --print, json = --mode json (not a pi flag)
  --print, -p                    Non-interactive mode: process prompt and exit
  --continue, -c                 Continue previous session
  --resume, -r                   Select a session to resume
  --session <path|id>            Use specific session file or partial UUID
  --session-id <id>              Use exact project session ID, creating it if missing
  --fork <path|id>               Fork specific session file or partial UUID into a new session
  --session-dir <dir>            Directory for session storage and lookup
  --no-session                   Don't save session (ephemeral)
  --name, -n <name>              Set session display name
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                                 Supports globs (anthropic/*, *sonnet*) and fuzzy matching
  --no-tools, -nt                Disable all tools by default (built-in and extension)
  --no-builtin-tools, -nbt       Disable built-in tools by default but keep extension/custom tools enabled
  --tools, -t <tools>            Comma-separated allowlist of tool names to enable
                                 Applies to built-in, extension, and custom tools
  --exclude-tools, -xt <tools>   Comma-separated denylist of tool names to disable
                                 Applies to built-in, extension, and custom tools
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh, max
  --extension, -e <path>         Load an extension file (can be used multiple times)
  --no-extensions, -ne           Disable extension discovery (explicit -e paths still work)
  --skill <path>                 Load a skill file or directory (can be used multiple times)
  --no-skills, -ns               Disable skills discovery and loading
  --prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
  --no-prompt-templates, -np     Disable prompt template discovery and loading
  --theme <path>                 Load a theme file or directory (can be used multiple times)
  --no-themes                    Disable theme discovery and loading
  --no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading
  --export <file>                Export session file to HTML and exit
  --list-models [search]         List available models (with optional fuzzy search)
  --verbose                      Force verbose startup (overrides quietStartup setting)
  --tui-mode <mode>              TUI mode: regular (default) or fullscreen
  --approve, -a                  Trust project-local files for this run
  --no-approve, -na              Ignore project-local files for this run
  --offline                      Disable startup network operations (same as CYRUP_OFFLINE=1)
  --help, -h                     Show this help
  --version, -v                  Show version number

Extensions can register additional flags (e.g., --plan from plan-mode extension).{ext_block}

Examples:
  # Print a provider API key for an external client
  {APP} auth print-api-key --provider openai --model gpt-5.5

  # Print an OAuth bearer token for an external client (refreshes if expired)
  {APP} auth print-bearer-token --provider openai-codex --model gpt-5.5

  # Interactive mode
  {APP}

  # Interactive mode with initial prompt
  {APP} \"List all .ts files in src/\"

  # Include files in initial message
  {APP} @prompt.md @image.png \"What color is the sky?\"

  # Non-interactive mode (process and exit)
  {APP} -p \"List all .ts files in src/\"

  # Multiple messages (interactive)
  {APP} \"Read package.json\" \"What dependencies do we have?\"

  # Continue previous session
  {APP} --continue \"What did we discuss?\"

  # Start a named session
  {APP} --name \"Refactor auth module\"

  # Use different model
  {APP} --provider openai --model gpt-4o-mini \"Help me refactor this code\"

  # Use model with provider prefix (no --provider needed)
  {APP} --model openai/gpt-4o \"Help me refactor this code\"

  # Use model with thinking level shorthand
  {APP} --model sonnet:high \"Solve this complex problem\"

  # Limit model cycling to specific models
  {APP} --models claude-sonnet,claude-haiku,gpt-4o

  # Limit to a specific provider with glob pattern
  {APP} --models \"github-copilot/*\"

  # Cycle models with fixed thinking levels
  {APP} --models sonnet:high,haiku:low

  # Start with a specific thinking level
  {APP} --thinking high \"Solve this complex problem\"

  # Read-only mode (no file modifications possible)
  {APP} --tools read,grep,find,ls -p \"Review the code in src/\"

  # Disable one tool while keeping the rest available
  {APP} --exclude-tools ask_question

  # Export a session file to HTML
  {APP} --export ~/{CFG}/agent/sessions/--path--/session.jsonl
  {APP} --export session.jsonl output.html

Environment Variables:
  ANTHROPIC_AUTH_TOKEN             - Anthropic bearer auth token
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ANTHROPIC_OAUTH_TOKEN            - Anthropic OAuth token (alternative to API key)
  ANT_LING_API_KEY                 - Ant Ling API key
  OPENAI_API_KEY                   - OpenAI GPT API key
  AZURE_OPENAI_API_KEY             - Azure OpenAI API key
  AZURE_OPENAI_BASE_URL            - Azure OpenAI/Cognitive Services base URL (e.g. https://{{resource}}.openai.azure.com)
  AZURE_OPENAI_RESOURCE_NAME       - Azure OpenAI resource name (alternative to base URL)
  AZURE_OPENAI_API_VERSION         - Azure OpenAI API version (default: v1)
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP - Azure OpenAI model=deployment map (comma-separated)
  DEEPSEEK_API_KEY                 - DeepSeek API key
  NVIDIA_API_KEY                   - NVIDIA NIM API key
  GEMINI_API_KEY                   - Google Gemini API key
  GROQ_API_KEY                     - Groq API key
  CEREBRAS_API_KEY                 - Cerebras API key
  XAI_API_KEY                      - xAI Grok API key
  FIREWORKS_API_KEY                - Fireworks API key
  TOGETHER_API_KEY                 - Together AI API key
  BASETEN_API_KEY                  - Baseten API key
  OPENROUTER_API_KEY               - OpenRouter API key
  AI_GATEWAY_API_KEY               - Vercel AI Gateway API key
  ZAI_API_KEY                      - ZAI Coding Plan API key (Global)
  ZAI_CODING_CN_API_KEY            - ZAI Coding Plan API key (China)
  MISTRAL_API_KEY                  - Mistral API key
  MINIMAX_API_KEY                  - MiniMax API key
  MOONSHOT_API_KEY                 - Moonshot AI API key
  OPENCODE_API_KEY                 - OpenCode Zen/OpenCode Go API key
  KIMI_API_KEY                     - Kimi For Coding API key
  CLOUDFLARE_API_KEY               - Cloudflare API token (Workers AI and AI Gateway)
  CLOUDFLARE_ACCOUNT_ID            - Cloudflare account id (required for both)
  CLOUDFLARE_GATEWAY_ID            - Cloudflare AI Gateway slug (required for AI Gateway)
  QWEN_TOKEN_PLAN_API_KEY          - Qwen Token Plan API key (international region)
  QWEN_TOKEN_PLAN_CN_API_KEY       - Qwen Token Plan API key (China region)
  XIAOMI_API_KEY                   - Xiaomi MiMo API key (api.xiaomimimo.com billing)
  XIAOMI_TOKEN_PLAN_CN_API_KEY     - Xiaomi MiMo Token Plan API key (China region)
  XIAOMI_TOKEN_PLAN_AMS_API_KEY    - Xiaomi MiMo Token Plan API key (Amsterdam region)
  XIAOMI_TOKEN_PLAN_SGP_API_KEY    - Xiaomi MiMo Token Plan API key (Singapore region)
  AWS_PROFILE                      - AWS profile for Amazon Bedrock
  AWS_ACCESS_KEY_ID                - AWS access key for Amazon Bedrock
  AWS_SECRET_ACCESS_KEY            - AWS secret key for Amazon Bedrock
  AWS_BEARER_TOKEN_BEDROCK         - Bedrock API key (bearer token)
  AWS_REGION                       - AWS region for Amazon Bedrock (e.g., us-east-1)
  {ENV_AGENT_DIR:<32} - Config directory (default: ~/{CFG}/agent)
  {ENV_SESSION_DIR:<32} - Session storage directory (overridden by --session-dir)
  CYRUP_PACKAGE_DIR                - Override package directory (for Nix/Guix store paths)
  CYRUP_OFFLINE                    - Disable startup network operations when set to 1/true/yes
  CYRUP_TELEMETRY                  - Override install telemetry when set to 1/true/yes or 0/false/no
  CYRUP_SHARE_VIEWER_URL           - Base URL for /share command (default: https://pi.dev/session/)

Built-in Tool Names:
  read       - Read file contents
  bash       - Execute bash commands
  powershell - Execute PowerShell commands on Windows
  edit       - Edit files with find/replace
  write      - Write files (creates/overwrites)
  grep       - Search file contents (read-only, off by default)
  find       - Find files by glob pattern (read-only, off by default)
  ls         - List directory contents (read-only, off by default)
"
    )
}
