use super::*;

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
    let with_ext = render_help(&[ExtensionFlag {
        name: "plan".into(),
        value: ExtFlagValue::Bool(true),
    }]);
    assert!(with_ext.contains("Extension CLI Flags:"));
    assert!(with_ext.contains("--plan"));
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
