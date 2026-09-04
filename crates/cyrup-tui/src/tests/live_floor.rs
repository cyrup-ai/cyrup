#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod live_floor_tests {
    use crate::UiTheme;
    use crate::app::*;
    use ratatui::backend::TestBackend;

    /// The FLICKER fix's height logic (a unit guard — the definitive check is the pty drive): while a
    /// turn is active the live-region height uses a grow-only floor so it does NOT track per-tool
    /// content churn (which is what forced a `resize_viewport`/`reanchor_inline` reconstruction — the
    /// flicker source — on essentially every tool event). The instant the turn goes idle the floor
    /// resets so the region collapses back to the compact editor/footer (void-fix).
    #[test]
    fn live_floor_grows_then_holds_during_a_turn_and_resets_when_idle() {
        let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
        app.status_mut().set_model("anthropic/claude-opus-4-8");
        app.draw().unwrap();
        let idle = app.viewport_height();

        // Turn goes active (AgentStart sets `status.streaming`); a burst of finished tools grows the
        // live tail before it is committed.
        app.status_mut().set_streaming(true);
        for i in 0..8u32 {
            let name = format!("read_{i}");
            app.transcript_mut().push_tool_start(
                name.clone(),
                serde_json::json!({ "path": format!("file_{i}.md") }),
            );
            app.transcript_mut().push_tool_end(
                name,
                false,
                Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("body {i}") }] })),
            );
        }
        app.draw().unwrap();
        let grown = app.viewport_height();
        assert!(
            grown > idle,
            "viewport should grow for the live tool tail ({grown} vs idle {idle})"
        );

        // The finished tools commit to native scrollback mid-turn (SCREEN-FILL fix): the live content
        // collapses — and with a commit PENDING FLUSH the floor RELEASES to the remaining content
        // height (TUI-090), so the flush lands on screen above the shrunken viewport instead of
        // invisibly in scrollback. The grow-only hold now applies only BETWEEN commits (nothing
        // pending flush); this is the one frame per commit where a shrink is visually required.
        app.transcript_mut().commit_finished_leading_tools();
        assert_eq!(
            app.state().transcript.active_tools().len(),
            0,
            "finished tools left the tail"
        );
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            idle,
            "with a commit pending flush and the live tail gone, the floor releases to the \
             remaining content height (the compact chrome) so the flush stays visible (TUI-090)"
        );

        // Turn goes idle (AgentEnd clears `status.streaming`): the floor resets and the region
        // collapses back to the compact idle height (void-fix preserved).
        app.status_mut().set_streaming(false);
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            idle,
            "idle viewport must collapse back to the compact region after the turn"
        );
    }

    /// The TUI-090 release's other half: it fires ONLY on frames that will flush a commit. Content
    /// shrink with NOTHING pending flush — e.g. the user deleting editor text mid-turn — must hold
    /// the floor, or every shrink would force a `resize_viewport`/`reanchor_inline` reconstruction
    /// for mere content churn: the per-event FLICKER the floor exists to kill.
    #[test]
    fn the_floor_holds_when_content_shrinks_with_nothing_pending_flush() {
        let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
        app.status_mut().set_model("anthropic/claude-opus-4-8");
        app.draw().unwrap();

        // A multi-line editor makes the live region taller without any transcript content; a live
        // tool tail pins the floor above that.
        app.editor_mut()
            .set_text("line 1\nline 2\nline 3\nline 4\nline 5");
        app.status_mut().set_streaming(true);
        for i in 0..8u32 {
            let name = format!("read_{i}");
            app.transcript_mut().push_tool_start(
                name.clone(),
                serde_json::json!({ "path": format!("file_{i}.md") }),
            );
            app.transcript_mut().push_tool_end(
                name,
                false,
                Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("body {i}") }] })),
            );
        }
        app.draw().unwrap();
        let grown = app.viewport_height();

        // The user deletes the editor text mid-turn: the live region's content shrinks, but no
        // commit has happened, so nothing is pending flush. The floor must HOLD.
        app.editor_mut().clear();
        assert!(
            app.state().transcript.pending().is_empty(),
            "no commit has happened, so nothing can be pending flush"
        );
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            grown,
            "the floor must hold across content shrink with nothing pending flush (FLICKER fix); \
             the TUI-090 release fires only on frames that will flush a commit"
        );
    }

    /// The release only fires DOWNWARD. On a flush frame whose REMAINING live content is taller
    /// than the pinned floor — a small tool tail commits while the streaming answer has already
    /// grown past it — `raw < live_floor` is false, nothing is stale, and the floor must grow to
    /// the content exactly as it does between commits. Releasing here would clip a growing turn.
    #[test]
    fn the_floor_grows_rather_than_releases_on_a_flush_frame_when_the_tail_grew() {
        let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
        app.status_mut().set_model("anthropic/claude-opus-4-8");
        app.draw().unwrap();

        // A small finished-tool tail pins the floor low.
        app.status_mut().set_streaming(true);
        for i in 0..2u32 {
            let name = format!("read_{i}");
            app.transcript_mut().push_tool_start(
                name.clone(),
                serde_json::json!({ "path": format!("file_{i}.md") }),
            );
            app.transcript_mut().push_tool_end(
                name,
                false,
                Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("body {i}") }] })),
            );
        }
        app.draw().unwrap();
        let low = app.viewport_height();

        // The tools commit (production order: `ToolExecutionEnd` commits while no assistant
        // message is streaming — `commit_finished_leading_tools` early-returns otherwise), and
        // the assistant text THEN streams tall ahead of the next frame: a flush IS pending, but
        // the remaining live content (the streaming partial) outgrows the pinned floor.
        app.transcript_mut().commit_finished_leading_tools();
        assert!(
            !app.state().transcript.pending().is_empty(),
            "the commit is pending flush on this frame"
        );
        app.transcript_mut()
            .push_assistant_delta(&"a streamed answer line\n".repeat(30));
        app.draw().unwrap();
        let grown = app.viewport_height();
        assert!(
            grown > low,
            "with a flush pending but the live tail TALLER than the floor, the floor must grow to \
             the content ({grown} > {low}) — the TUI-090 release only fires downward"
        );
    }
}
