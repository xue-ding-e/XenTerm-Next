"""Source wiring regression, not a substitute for GPUI/macOS execution.

An optional repository root lets this exact check run against the pre-fix tree.
"""
from pathlib import Path
import sys
import unittest

ROOT = Path(sys.argv.pop(1)) if len(sys.argv) > 1 else Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "src/ui/impls/shell.rs").read_text()


class MacosReopenWiring(unittest.TestCase):
    def assertIn(self, needle, haystack):
        self.assertTrue(needle in haystack, f"missing wiring: {needle}")

    def test_platform_reopen_is_registered_before_run(self):
        self.assertIn("application.on_reopen(", SOURCE)
        self.assertLess(SOURCE.index("application.on_reopen("), SOURCE.index(".run(move |cx|"))

    def test_launch_and_reopen_share_one_window_path(self):
        self.assertIn("show_main_window(&main_window, &runtime_arc, &store, cx)", SOURCE)
        self.assertIn("show_main_window(&reopen_window, &reopen_runtime, &reopen_store, cx)", SOURCE)
        self.assertIn("main_window.set(Some(open_main_window(runtime, store, cx)?))", SOURCE)
        self.assertIn("window.activate_window()", SOURCE.split("fn show_main_window(")[1].split("fn open_main_window(")[0])

    def test_reopen_keeps_loaded_profile_and_runtime(self):
        self.assertEqual(SOURCE.count("crate::config::ConfigStore::load()"), 1)
        self.assertEqual(SOURCE.count("tokio::runtime::Builder::new_multi_thread()"), 1)
        self.assertIn("let reopen_store = store.clone();", SOURCE)
        self.assertIn("let reopen_runtime = runtime.clone();", SOURCE)

    def test_close_retires_prompts_and_sessions_without_waiting_for_quit(self):
        self.assertIn("cx.on_release(|shell: &mut Shell, cx|", SOURCE)
        teardown = SOURCE.split("cx.on_release(|shell: &mut Shell, cx|", 1)[1].split(".detach();", 1)[0]
        self.assertIn("crate::session::abort_window(WINDOW_ID)", teardown)
        self.assertIn("handle.commands.send(SessionCommand::Close)", teardown)
        self.assertIn("shell.state.end_tab(&tab)", teardown)

    def test_close_policy_and_explicit_quit_are_not_overridden(self):
        self.assertNotIn(".with_quit_mode(", SOURCE)
        self.assertNotIn(".set_quit_mode(", SOURCE)
        self.assertNotIn("on_window_should_close", SOURCE)
        self.assertEqual(SOURCE.count("cx.quit();"), 1, "keep the fatal startup error path")


if __name__ == "__main__":
    unittest.main()
