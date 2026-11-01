#!/usr/bin/env python3
"""Require every new SFTP interaction regression to actually run and pass.

Existing neighboring tests may also run. This fixed inventory is independent
of runtime discovery, so a renamed/deleted/ignored regression cannot pass as 0.
"""
import re
import sys
from collections import Counter
from pathlib import Path

EXPECTED = frozenset("""
ui::sftp_panel::navigation::tests::absolute_and_unicode_paths_are_not_reinterpreted
ui::sftp_panel::navigation::tests::relative_paths_use_the_visible_directory_and_keep_symlink_semantics
ui::sftp_panel::navigation::tests::blank_and_control_inputs_do_not_navigate
ui::sftp_panel::navigation::tests::shell_syntax_is_literal_and_backslash_is_not_a_local_separator
ui::sftp_panel::tests::path_enter_submits_once_without_changing_the_successful_directory
ui::sftp_panel::tests::path_escape_discards_the_draft_without_navigation_or_history
ui::sftp_panel::tests::path_edit_rejects_a_replaced_connection_or_changed_listing
ui::sftp_panel::tests::path_timeout_explains_recovery_without_admitting_another_navigation
ui::sftp_panel::tests::path_chinese_composition_needs_a_separate_enter_after_ime_commit
ui::sftp_panel::tests::external_file_drop_queues_exact_paths_once_and_is_not_a_dock_drag
ui::sftp_panel::tests::abandoned_external_drag_and_invalid_payload_do_not_upload
ui::sftp_panel::tests::drop_rejects_loading_directory_and_unsubmitted_path_draft
ui::shell::sftp_editor_routing_tests::navigation::entered_path_waits_for_success_and_permission_failure_keeps_the_old_directory
ui::shell::sftp_editor_routing_tests::navigation::disconnected_entered_navigation_does_not_change_position
ui::shell::sftp_editor_routing_tests::navigation::replaced_transport_rejects_a_draft_even_if_the_listing_is_identical
ui::shell::sftp_editor_routing_tests::navigation::switching_tabs_never_sends_the_old_path_to_the_new_session
ui::sftp_panel::drop_admission::tests::drop_preserves_unicode_and_order_while_deduplicating_the_same_path
ui::sftp_panel::drop_admission::tests::drop_rejects_empty_relative_and_over_limit_payloads_as_a_whole
ui::shell::sftp_editor_routing_tests::drops::drop_routes_only_to_the_visible_sftp_directory_without_legacy_overwrite
ui::shell::sftp_editor_routing_tests::drops::drop_rechecks_the_active_session_between_native_submit_and_shell_drain
ui::shell::sftp_editor_routing_tests::drops::drop_to_a_disconnected_transport_does_not_fallback_or_change_directory
ui::shell::sftp_editor_routing_tests::drops::drop_rejects_a_replaced_connection_before_shell_drain
ui::shell::sftp_editor_routing_tests::drops::drop_rejects_a_changed_listing_before_shell_drain
ui::shell::sftp_editor_routing_tests::drops::drop_on_the_terminal_never_becomes_pty_input_or_an_upload
sftp::sftp::drop_upload::policy::tests::unicode_spaces_and_shell_syntax_stay_literal
sftp::sftp::drop_upload::policy::tests::malformed_sources_and_destinations_are_rejected
sftp::sftp::drop_upload::policy::tests::duplicate_remote_names_remain_reserved_until_completion
sftp::sftp::drop_upload::policy::tests::queue_capacity_is_global_across_batches_and_reusable
sftp::sftp::drop_upload::tests::exclusive_open_success_requires_write_and_close_ack
sftp::sftp::drop_upload::tests::existing_name_has_no_write_remove_or_overwrite_fallback
sftp::sftp::drop_upload::tests::permission_denial_has_no_fallback
sftp::sftp::drop_upload::tests::cancelled_before_open_sends_no_file_request
sftp::sftp::drop_upload::tests::cancel_after_create_closes_without_write_or_remove
sftp::sftp::drop_upload::tests::cancel_after_write_closes_and_retains_partial
sftp::sftp::drop_upload::tests::write_failure_is_not_completion_and_still_closes
sftp::sftp::drop_upload::tests::close_failure_cannot_report_completed
sftp::sftp::drop_upload::tests::disconnect_cannot_report_completed
sftp::sftp::drop_upload::tests::queued_cancel_wakes_without_waiting_for_a_slot
sftp::sftp::drop_upload::tests::only_two_transfers_acquire_slots_and_cancel_releases_reservations
sftp::sftp::drop_upload::tests::worker_shutdown_cancels_and_wakes_queued_drops
sftp::sftp::drop_upload::tests::local_directories_and_links_are_rejected_regular_files_are_read
core::transfer::tests::cancelled_detail_preserves_retained_partial_warning
core::transfer::tests::completed_detail_preserves_refresh_hint_without_changing_progress
""".split())
assert len(EXPECTED) == 43
RESULT = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)(?: .*)?$", re.MULTILINE)
SUMMARY = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", re.MULTILINE)


def verify(logs):
    seen = Counter()
    for text in logs:
        summaries = SUMMARY.findall(text)
        if not summaries or not any(int(passed) > 0 for _, passed, _ in summaries):
            raise ValueError("missing/nonexecuting test summary")
        if any(result != "ok" or int(failed) != 0 for result, _, failed in summaries):
            raise ValueError("a test process reported failure")
        for name, result in RESULT.findall(text):
            if name in EXPECTED:
                if result != "ok":
                    raise ValueError(f"required test did not pass: {name}")
                seen[name] += 1
    if set(seen) != EXPECTED or any(count != 1 for count in seen.values()):
        raise ValueError("required inventory missing, duplicated, or substituted")
    return len(seen)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: sftp_interaction_inventory.py SFTP_LOG TRANSFER_LOG")
    print(f"SFTP interaction inventory: {verify([Path(p).read_text() for p in sys.argv[1:]])} exact tests passed")
