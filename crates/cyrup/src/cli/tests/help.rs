use super::*;
use cyrup_session_svc::ExtensionFlagDeclaration;

#[test]
fn help_body_contains_pi_catalogue_examples_and_tools() {
    let help = render_help(&[]);
    assert!(help.contains("Environment Variables:"));
    assert!(help.contains("ANTHROPIC_API_KEY"));
    assert!(help.contains("TOGETHER_API_KEY"));
    assert!(help.contains("CYRUP_AGENT_DIR"));
    assert!(help.contains("Built-in Tool Names:"));
    assert!(help.contains("Examples:"));
    assert!(help.contains("cyrup install <source>"));
    // Extension flags inject into the body when present.
    let with_ext = render_help(&[declared("plan", "boolean", Some("Plan first"))]);
    assert!(with_ext.contains("Extension CLI Flags:"));
    assert!(with_ext.contains("--plan"));
}

fn declared(name: &str, ty: &str, description: Option<&str>) -> ExtensionFlagDeclaration {
    ExtensionFlagDeclaration {
        name: name.to_string(),
        description: description.map(str::to_string),
        flag_type: Some(ty.to_string()),
        default: None,
        extension: cyrup_sdk::core::ExtensionId::from("demo-ext"),
    }
}

/// SEAM-020 (c) — pi's extension-flag rows (`printHelp`, `cli/args.ts:262-270` @v0.87.1):
/// `` `  --${flag.name}${value}`.padEnd(30) + (flag.description ?? `Registered by ${flag.extensionPath}`) ``,
/// in declaration order, ` <value>` only for a `"string"` flag. cyrup printed the bare
/// `  --name` head with no description column at all.
#[test]
fn extension_flag_rows_are_pis_padded_head_and_description() {
    let help = render_help(&[
        declared("mcp-config", "string", Some("Path to MCP config file")),
        declared("plan", "boolean", None),
        declared(
            "a-flag-name-longer-than-the-column",
            "boolean",
            Some("Tight"),
        ),
    ]);
    let block = help
        .split("Extension CLI Flags:\n")
        .nth(1)
        .expect("the extension flag block");
    let rows: Vec<&str> = block.lines().take(3).collect();
    assert_eq!(
        rows,
        vec![
            "  --mcp-config <value>        Path to MCP config file",
            "  --plan                      Registered by demo-ext",
            "  --a-flag-name-longer-than-the-columnTight",
        ]
    );
}

/// CFG-068 — every directory variable cyrup reads must be visible in one place, and the set is
/// FOUR, not three. `docs/guide/reference/environment.md:56-64`'s section *"The three directory
/// variables that are not synonyms"* names `CYRUP_AGENT_DIR`, `CYRUP_CODING_AGENT_DIR` and
/// `CYRUP_HOME` — the long agent-dir spelling is one of its three and `CYRUP_SESSION_DIR` is not.
/// An earlier version of this test was called `help_names_all_three_directory_variables` and
/// asserted `CYRUP_SESSION_DIR` in `CYRUP_CODING_AGENT_DIR`'s place, so it claimed to pin the
/// guide's three while `--help` documented only two of them.
///
/// The long spelling is not a cyrup extra: upstream has ONE agent-dir variable and it IS the long
/// one — `config.ts:508` @v0.87.1 `export const ENV_AGENT_DIR = \`${APP_NAME.toUpperCase()}_CODING_AGENT_DIR\``
/// — and `cli/args.ts:441` renders exactly that constant, `${ENV_AGENT_DIR.padEnd(32)}`, in the
/// `Environment Variables:` block. cyrup's rename split pi's one name into two spellings
/// (`paths::ENV_AGENT_DIR_KEYS`) and advertised only the short one, so the variable whose upstream
/// counterpart `--help` is *supposed* to list was the missing row.
///
/// Each name is matched as a whole RENDERED ROW, not as a bare substring: `help.contains("CYRUP_HOME")`
/// is also satisfied by a row spelled `CYRUP_HOME_DIR`, so a substring assertion cannot catch a row
/// that has drifted away from the variable the reader uses — which is the failure mode this row
/// exists to close. Membership rather than a whole-body comparison, because the `ext_block` makes
/// the body variable-length.
#[test]
fn help_names_the_guides_three_non_synonyms_and_the_session_dir() {
    let help = render_help(&[]);
    for name in [
        cyrup_config::paths::ENV_AGENT_DIR,
        cyrup_config::paths::ENV_CODING_AGENT_DIR,
        cyrup_config::paths::ENV_HOME,
        cyrup_config::paths::ENV_SESSION_DIR,
    ] {
        // pi's own row shape: two spaces, the constant padded to 32 (`args.ts:441`), then ` - `.
        let row = format!("\n  {name:<32} - ");
        assert!(
            help.contains(&row),
            "{name} relocates a tree and must have its own --help row: {help}"
        );
    }
}

/// CFG-068's other half, and the one the row was actually filed about: a `--help` row must name a
/// variable something READS. `CYRUP_HOME` was read at four sites and advertised nowhere; the
/// symmetric failure is a row advertised here and read nowhere (the `CYRUP_SHARE_VIEWER_URL`
/// history in `the_env_help_block_and_the_read_set_are_the_same_set` below).
///
/// A `help.contains("CYRUP_HOME")` assertion cannot tell those apart — a hand-typed literal in the
/// help body satisfies it either way. So each of the four names is driven through
/// [`cyrup_config::env::EnvVars::from_lookup`], cyrup-config's single env touchpoint, and the
/// resolved field is checked: that is the reader, not a restatement of the help text. The help body
/// itself now takes these strings from the same `cyrup_config::paths` constants, so the two sides
/// cannot be spelled differently.
#[test]
fn each_directory_row_names_a_variable_a_reader_honours() {
    use cyrup_config::paths;
    use std::path::Path;

    let only = |set: &'static str| {
        move |key: &str| (key == set).then(|| std::ffi::OsString::from("/sentinel"))
    };

    let home = cyrup_config::env::EnvVars::from_lookup(only(paths::ENV_HOME));
    assert_eq!(
        home.home.as_deref(),
        Some(Path::new("/sentinel")),
        "{} must reach the home ladder",
        paths::ENV_HOME
    );
    // …and it must NOT move configuration, which is the clause the help row carries.
    assert_eq!(
        home.agent_dir, None,
        "CYRUP_HOME does not move the agent dir"
    );

    for key in [paths::ENV_AGENT_DIR, paths::ENV_CODING_AGENT_DIR] {
        let vars = cyrup_config::env::EnvVars::from_lookup(only(key));
        assert_eq!(
            vars.agent_dir.as_deref(),
            Some(Path::new("/sentinel")),
            "{key} must reach the agent-dir ladder"
        );
    }

    let session = cyrup_config::env::EnvVars::from_lookup(only(paths::ENV_SESSION_DIR));
    assert_eq!(
        session.session_dir.as_deref(),
        Some(Path::new("/sentinel")),
        "{} must reach the session-dir override",
        paths::ENV_SESSION_DIR
    );
}

/// SEAM-111 — the Commands block against pi's `args.ts:226-235`, on the three clauses that had
/// drifted. Two of them UNDERSTATED the shipped surface: `-l` and the Tab hint describe behaviour
/// that has always worked, and the model-catalog clause became true when SEAM-100 landed
/// `cyrup update --models`.
#[test]
fn the_top_level_commands_block_states_the_shipped_surface() {
    let help = render_help(&[]);
    assert!(
        help.contains("cyrup config [-l]"),
        "`-l` ships (subcommands.rs's config arm) but was unadvertised: {help}"
    );
    assert!(
        help.contains("(Tab switches scope)"),
        "Tab switches write scope in the picker (pi args.ts:234): {help}"
    );
    assert!(
        help.contains("Update cyrup, extensions, or model catalogs"),
        "pi's `update` clause names all three targets (args.ts:232), and `--models` now exists"
    );
    // The clause is only honest because the command it names is real.
    assert!(
        crate::subcommands::render_command_help(crate::subcommands::PackageCommand::Update)
            .contains("--models")
    );
}

/// The env block and the read set must be the SAME set, in both directions (SEAM-102 /
/// TUI-063 — one invariant, two failure modes).
///
/// Direction 1 (SEAM-102, seven rows): a credential cyrup genuinely reads was missing from the
/// block, so `--help` told a user with a working `ANTHROPIC_AUTH_TOKEN` / Qwen / Xiaomi key that
/// cyrup does not read it. Each name is asserted against
/// [`cyrup_provider::env_api_keys::api_key_env_vars`], the table the resolver itself consults,
/// so the row cannot be right in the help and wrong in the product.
///
/// Direction 2 (TUI-063): `CYRUP_SHARE_VIEWER_URL` was advertised and read by nothing. It has a
/// consumer now (`cyrup-tui`'s `/share`, `share_viewer_url`), and pi's row carries the default
/// (`args.ts:389` @v0.83.0) — which the cyrup row had dropped, leaving the help unable to say
/// what happens when the variable is unset.
#[test]
fn the_env_help_block_and_the_read_set_are_the_same_set() {
    let help = render_help(&[]);
    for (provider, name) in [
        ("anthropic", "ANTHROPIC_AUTH_TOKEN"),
        ("qwen-token-plan", "QWEN_TOKEN_PLAN_API_KEY"),
        ("qwen-token-plan-cn", "QWEN_TOKEN_PLAN_CN_API_KEY"),
        // PROV-014: the Individual plan shares the international variable (env-api-keys.ts:83
        // @v0.84.4), so pi's block needs no third Qwen row (`args.ts:419-420` @v0.84.4) — and
        // `RADIUS_API_KEY` is deliberately NOT asserted: pi's block does not list it either.
        ("qwen-token-plan-individual", "QWEN_TOKEN_PLAN_API_KEY"),
        ("xiaomi", "XIAOMI_API_KEY"),
        ("xiaomi-token-plan-cn", "XIAOMI_TOKEN_PLAN_CN_API_KEY"),
        ("xiaomi-token-plan-ams", "XIAOMI_TOKEN_PLAN_AMS_API_KEY"),
        ("xiaomi-token-plan-sgp", "XIAOMI_TOKEN_PLAN_SGP_API_KEY"),
        // DRIFT-009: pi's block carries this row too (`args.ts:406` @v0.84.4), between
        // TOGETHER_API_KEY and OPENROUTER_API_KEY, which is where cyrup's now sits.
        ("baseten", "BASETEN_API_KEY"),
    ] {
        assert!(
            cyrup_provider::env_api_keys::api_key_env_vars(provider)
                .is_some_and(|keys| keys.contains(&name)),
            "{name} must really be read for {provider}, or the help row would be the lie in the \
                 other direction"
        );
        assert!(
            help.contains(name),
            "{name} is read but absent from the --help environment block"
        );
    }
    assert!(
            help.contains(
                "CYRUP_SHARE_VIEWER_URL           - Base URL for /share command (default: https://pi.dev/session/)"
            ),
            "pi's row carries the default (args.ts:389): {help}"
        );
}

/// SEAM-119 — pi's help row for `--use-theme`, verbatim, directly under `--theme`
/// (`cli/args.ts:318-319` @v0.87.1).
#[test]
fn help_lists_use_theme_under_theme() {
    let help = render_help(&[]);
    let theme_row = "  --theme <path>                 Load a theme file or directory (can be used multiple times)\n";
    let use_theme_row =
        "  --use-theme <name[/name]>      Set the initial interactive theme for this run\n";
    assert!(
        help.contains(&format!("{theme_row}{use_theme_row}")),
        "{help}"
    );
}
