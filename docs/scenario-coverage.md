# Scenario Coverage

Status: fake-MLS reducer scenarios, SQLite restart scenarios, and the first
OpenMLS credential spike passing.

Run:

```sh
cargo test -p finitechat-sim --test scenarios
cargo test -p finitechat-store --test sqlite_scenarios
cargo test -p finitechat-mls
```

## Proven Scenarios

Each item below has a named test in
`crates/finitechat-sim/tests/scenarios.rs`.

- `create_dm_room_and_release_welcome_after_commit`
- `welcome_activation_makes_new_device_active`
- `duplicate_commit_retry_returns_same_result_after_side_effects`
- `conflicting_idempotency_key_rejects_without_side_effects`
- `same_epoch_loser_restart_retry_replays_rejection`
- `welcome_is_not_released_before_accepted_commit`
- `key_package_lease_expiry_returns_package_to_available`
- `consumed_key_package_cannot_be_reused`
- `stale_key_package_ref_is_rejected_without_side_effects`
- `invalid_commit_report_fails_closed`
- `membership_delta_disagreement_enters_needs_repair`
- `false_remove_delta_does_not_block_removed_device_from_validating_removal_seq`
- `new_device_linking_partial_failure_retries_only_failed_room`
- `link_mailbox_payload_is_opaque_to_server_state`
- `link_session_duplicate_conflict_expiry_and_delivery_rules`
- `push_wake_is_only_a_hint_and_does_not_advance_client_state`
- `stale_push_for_removed_device_cannot_authorize_new_events`
- `accepted_commit_response_lost_then_server_restart_replays_same_result`
- `commit_durable_before_welcome_release_restart_releases_exactly_once`
- `commit_effects_are_atomic_at_reducer_boundary`
- `welcome_claim_crash_before_ack_can_resume_after_restart`
- `delayed_welcome_after_later_entries_syncs_forward_from_commit_seq`
- `welcome_terminal_failure_keeps_membership_interval_inactive`
- `fetch_then_stream_gap_is_repaired_by_pull_cursor`
- `stable_message_id_survives_retry_and_distinguishes_payloads`
- `membership_delta_structural_matrix_rejects_bad_shapes`
- `direct_room_create_or_get_and_third_account_rejection`
- `fake_device_credential_validation_rejects_wrong_bindings`
- `fake_welcome_missing_ratchet_tree_fails_activation`
- `login_challenge_replay_rules_are_single_use`
- `local_pending_commit_is_not_merged_until_server_log_observed`
- `fake_changed_leaf_credential_validation_uses_same_device_binding_rules`
- `link_fanout_existing_device_stale_isolated_to_failed_room`
- `oversized_application_payload_is_rejected_without_log_entry`
- `sync_events_returns_bounded_page`
- `duplicate_message_id_with_new_idempotency_key_is_rejected`
- `idempotency_capacity_rejects_new_mutations_but_allows_replay`
- `direct_room_rejects_too_many_devices_for_one_account`

## Meaning Of Fake-MLS

These tests prove protocol ordering, idempotency, delivery, and state-machine
rules before real MLS is wired in. They intentionally do not prove OpenMLS
cryptographic correctness.

Some OpenMLS-specific scenarios are represented as fake validation gates for
now:

- device credential validation;
- changed LeafNode credential validation;
- Welcome activation requiring ratchet-tree material;
- local pending Commit merge only after server-log observation.

Those gates become real OpenMLS tests in Phase 2.

## OpenMLS Credential Proof

The first real OpenMLS-facing tests live in `crates/finitechat-mls/src/lib.rs`.

Proven credential scenarios:

- `nostr_signed_device_credential_verifies`
- `wrong_account_key_rejects`
- `wrong_device_id_rejects`
- `wrong_mls_leaf_key_rejects`
- `tampered_signature_payload_rejects`
- `expired_credential_rejects`
- `not_yet_valid_credential_rejects`
- `invalid_sizes_reject_before_signing`
- `openmls_basic_credential_round_trips_finite_identity_bytes`
- `openmls_key_package_carries_nostr_rooted_device_credential`
- `openmls_welcome_adds_device_after_server_ordered_commit_merge`
- `openmls_welcome_without_ratchet_tree_material_rejects`
- `real_openmls_bytes_flow_through_engine_ordering`

This proves the identity refinement from the protocol docs: OpenMLS carries the
credential bytes, but Finite Chat clients verify the Nostr-rooted account,
device id, and MLS leaf signing key locally. The server can order room entries
without deciding who a device is.

Checkpoint test signal:

- The fake-MLS pending-Commit rule mapped cleanly to OpenMLS: `add_members`
  leaves a pending local Commit and does not advance the sender epoch until
  `merge_pending_commit`.
- The first real Welcome test exposed an OpenMLS storage behavior the fake
  reducer could not model: trying to stage a Welcome without ratchet-tree
  material can consume the local KeyPackage before failure. Production clients
  should persist the Welcome and wait for tree material before invoking OpenMLS
  Welcome staging.
- The credential tests were still relevant: no Nostr binding code changed when
  the OpenMLS provider/signer boundary was added.
- The first engine-through-MLS test caught a ratchet-tree timing mistake:
  exporting Alice's tree before the server-observed Commit merge produced
  OpenMLS `TreeHashMismatch`. The correct production rule is stricter than the
  fake reducer could express: publish or serve ratchet-tree material from the
  accepted post-Commit group state.
- The current engine proof keeps Welcome bytes and ratchet-tree bytes in the
  client test harness because `DeliveryService` currently persists only Welcome
  release metadata. That is an intentional exposed gap, not a hidden server
  abstraction. A later checkpoint should add an explicit opaque Welcome payload
  store before finitecomputer integration.

## SQLite Follow-Up

The SQLite suite lives in
`crates/finitechat-store/tests/sqlite_scenarios.rs`.

Proven SQLite restart scenarios:

- `sqlite_create_dm_room_and_release_welcome_after_commit`
- `sqlite_duplicate_commit_retry_after_reopen_returns_same_result`
- `sqlite_rejected_commit_is_replayable_after_reopen`
- `sqlite_conflicting_idempotency_key_has_no_side_effects`
- `sqlite_welcome_not_released_before_accepted_commit`
- `sqlite_key_package_lease_expiry_and_reclaim_survives_reopen`
- `sqlite_consumed_key_package_cannot_be_reused`
- `sqlite_removed_device_can_sync_through_removal_after_reopen`
- `sqlite_invalid_commit_report_blocks_room_after_reopen`
- `sqlite_welcome_claim_crash_before_ack_resumes_after_reopen`
- `sqlite_delayed_welcome_syncs_forward_from_commit_seq`
- `sqlite_terminal_welcome_failure_keeps_interval_inactive`
- `sqlite_link_session_state_machine_survives_reopen`
- `sqlite_direct_room_create_or_get_and_third_account_rejection`
- `sqlite_oversized_application_payload_is_rejected_without_persisting_log`
- `sqlite_sync_events_returns_bounded_page_after_reopen`
- `sqlite_duplicate_message_id_is_typed_engine_error`
- `sqlite_link_payload_limit_is_rejected`
- `sqlite_idempotency_capacity_rejects_new_mutations_but_allows_replay`
- `sqlite_commit_crash_matrix_rolls_back_and_retry_converges`
- `sqlite_commit_epoch_unique_index_blocks_second_commit_row`

The SQLite crash matrix injects transaction rollbacks after log append, room
head update, removed membership update, added membership insert, KeyPackage
consumption, Welcome release, and idempotency record insert. It then reopens the
store, retries the same Commit, and verifies convergence to one log entry, one
epoch advance, correct membership intervals, consumed KeyPackages, released
Welcomes, and a replayable idempotency result.

Push outbox rows are not implemented yet; when they land, this matrix should add
a failure point after outbox enqueue and assert exactly one durable wake record.
