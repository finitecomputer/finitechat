# Scenario Coverage

Status: fake-MLS reducer scenarios, SQLite restart scenarios, OpenMLS
credential proof, and the reusable OpenMLS client proof passing.

Run:

```sh
cargo test -p finitechat-sim --test scenarios
cargo test -p finitechat-store --test sqlite_scenarios
cargo test -p finitechat-mls
cargo test -p finitechat-client
```

## Proven Scenarios

Each item below has a named test in
`crates/finitechat-sim/tests/scenarios.rs`.

- `create_dm_room_and_release_welcome_after_commit`
- `key_package_claim_returns_opaque_payload`
- `account_key_package_claim_returns_one_available_package_per_device`
- `revoked_device_cannot_replenish_or_claim_key_packages`
- `key_package_inventory_is_bounded_and_consumed_packages_free_space`
- `revoked_device_cannot_claim_or_activate_pending_welcome`
- `revoked_active_device_cannot_send_or_commit`
- `welcome_activation_makes_new_device_active`
- `add_commit_requires_staged_welcome_bytes_before_mutation`
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
- `multi_device_pending_invite_action_order_fuzz_keeps_server_roles_separate`

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
- `nostr_secret_derivation_is_stable_and_domain_separated`
- `nostr_secret_derivation_rejects_unbounded_input`
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

## OpenMLS Client Proof

The production-shaped client tests live in
`crates/finitechat-client/tests/client_state.rs`.

Proven client scenarios:

- `client_state_machine_adds_device_and_decrypts_application_message`
- `multi_device_invite_late_joiner_catches_up_to_new_messages`
- `multi_device_real_mls_ordering_matrix_validates_late_catch_up`
- `sqlite_client_state_survives_restart_for_late_multi_device_catch_up`
- `sqlite_client_store_encrypts_state_and_rejects_wrong_or_tampered_key_material`
- `sqlite_client_welcome_activation_is_durable_before_server_ack`
- `sqlite_client_claimed_welcome_survives_restart_before_activation`
- `sqlite_client_failed_pending_welcome_activation_keeps_inbox_entry`
- `sqlite_client_apply_log_entry_persists_cursor_and_skips_replay_after_restart`
- `client_processes_remote_add_commit_before_epoch_two_messages`
- `client_processes_remote_update_commit_before_epoch_three_messages`
- `client_processes_remote_remove_commit_before_post_remove_messages`
- `stale_removed_device_can_process_removal_but_not_future_ciphertext`
- `client_recovers_losing_same_epoch_add_commit_and_retries`
- `client_recovers_losing_same_epoch_update_commit_and_retries`
- `client_recovers_losing_same_epoch_remove_commit_and_retries`
- `client_drops_losing_pending_commit_when_winning_race_removes_it`
- `client_key_package_replenishment_edges_use_real_packages`
- `client_key_package_replenishment_plan_maintains_bounded_inventory`
- `runtime_sync_tick_replenishes_welcomes_acks_and_syncs_after_restart`
- `runtime_sync_tick_retries_key_package_upload_after_response_loss`
- `new_device_history_policy_starts_at_add_commit_not_prior_messages`
- `client_links_new_device_into_existing_rooms_with_distinct_key_packages`
- `sqlite_link_fanout_worker_survives_restart_after_prepared_commit`
- `runtime_link_fanout_tick_links_later_device_after_submit_response_loss`
- `runtime_link_fanout_tick_reprepares_after_same_epoch_loss`
- `client_link_fanout_rejects_wrong_claim_before_pending_commit`
- `client_rejects_tampered_remote_commit_without_epoch_advance`
- `client_refuses_to_merge_pending_commit_before_server_observation`
- `client_rejects_invalid_invite_request_before_local_pending_commit`
- `client_rejects_tampered_ratchet_tree_before_ack`

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
- The Welcome payload checkpoint closed the exposed server gap: `submit_commit`
  now requires staged Welcome and ratchet-tree bytes for every add, the engine
  and SQLite store return those exact bytes on claim, and the real OpenMLS test
  stages Bob from server-delivered bytes instead of a test-harness side channel.
- The real MLS proof also found the right OpenMLS API shape: Alice exports the
  post-Commit ratchet tree from the pending commit without merging local state,
  so the client can submit bytes to the server while still waiting for ordered
  Commit acceptance before `merge_pending_commit`.
- The client checkpoint removed the raw engine/OpenMLS harness and moved that
  behavior into `finitechat-client`: KeyPackage bytes are claimed from server
  storage, Welcome bytes are claimed from server storage, Alice refuses app
  sends while a local Commit is pending, and Bob decrypts a finitecomputer-style
  JSON command after acking the Welcome.
- The multi-device checkpoint confirmed the earlier interval model was the
  right one: devices added by an accepted Commit can sync entries after that
  Commit even before they ack their Welcome, while the server still rejects
  sends until each device's Welcome is acked. The real OpenMLS test then proved
  a late Alice device can activate its batch Welcome and decrypt messages sent
  before it joined locally.
- The heavy real-MLS matrix replays that same invariant across all activation
  orders for three Alice devices and several Bob-message timing patterns. It
  stays in the normal test suite because it runs quickly enough to catch MLS
  ordering regressions before they reach integration work.
- The first client SQLite restart proof persists OpenMLS storage rows, the
  device profile, and room mappings. It reloads Bob before sending, reloads
  Alice browser after activation, and reloads a late Alice phone before it
  decrypts messages sent while it was pending.
- The encrypted client-store checkpoint replaces those raw local tables with a
  Nostr-derived encrypted snapshot. The restart proof still passes, and the new
  negative test checks that legacy cleartext tables are absent, sampled raw
  credential/OpenMLS bytes are not stored in the ciphertext, the wrong derived
  key cannot load the device, and tampering fails closed.
- The crash-resume checkpoint moves applied room cursors into the encrypted
  client snapshot and adds store-backed operations for Welcome activation and
  ordered-log apply. Bob can activate and persist a Welcome before server ack,
  restart, then ack and decrypt future messages. Bob can also process a remote
  Commit and an application message through the store, restart with the cursor
  already advanced, and skip replayed entries without asking OpenMLS to process
  an already-applied epoch/message again.
- The pending-Welcome checkpoint covers the remaining claim/activation crash
  window: after Bob claims a Welcome, the server no longer returns it from
  `claim_welcomes`; Bob persists the Welcome payload and ratchet tree in the
  encrypted client snapshot, restarts, activates from local state, clears the
  pending inbox entry, and then acks the server. Activated Welcomes now leave
  durable pending-ack state until the server ack succeeds; server ack is
  idempotent, so a crash after ack but before clearing local ack state can retry.
  A companion failure test corrupts the stored ratchet tree and proves OpenMLS
  rejection does not drop the only local pending-Welcome copy.
- The first remote Commit checkpoint adds a real ordered-log client API:
  application entries decrypt, own Commit entries merge only with pending local
  state, and remote Commit entries validate the log envelope before processing
  the OpenMLS staged Commit. The valid test advances Bob from epoch 1 to epoch
  2 after Alice adds Charlie, and the invalid test rejects tampered Commit bytes
  without advancing Bob's epoch.
- The remove/update checkpoint extends that same API instead of adding a second
  path: clients can now produce real OpenMLS self-update and remove Commits,
  submit empty-delta update Commits or remove deltas to the server, merge their
  own ordered Commit, and process another device's ordered Commit before
  accepting post-epoch messages. The remove proof also checks that the removed
  device can process its removal Commit, then cannot send locally or receive
  post-remove server events.
- The later-device-link checkpoint proves the thick client responsibility across
  more than one room: Alice has two existing rooms, a newly linked Alice phone
  uploads distinct KeyPackages for each room, existing room members add that
  phone with separate accepted Commits, and the phone activates both Welcomes
  before decrypting post-link messages in both rooms. This keeps KeyPackage
  single-use behavior visible instead of hiding it behind UI orchestration.
- The fanout-discovery checkpoint adds the server-side shape that a durable
  link worker needs next: account-room discovery is paged, includes
  current/pending devices for the account, survives SQLite reopen, and duplicate
  current/pending device adds are rejected before a retry can consume a leased
  KeyPackage or release another Welcome. The same checkpoint also makes the
  group-room devices-per-account cap executable, while direct rooms keep their
  tighter cap.
- The durable link-fanout worker checkpoint closes the client crash boundary
  around local pending Commits: Alice queues room plans from account discovery,
  prepares an add Commit for one room, persists the prepared server request with
  encrypted MLS state, restarts before submit, submits the recovered request,
  completes from the ordered log, and repeats for a second room before the new
  Alice device activates both Welcomes. A negative test passes a KeyPackage
  claim for the wrong target and proves no local pending Commit is created.
- The runtime link-fanout checkpoint moves that sequence behind the API
  finitecomputer should drive. The target device replenishes real MLS
  KeyPackages through the runtime sync tick; Alice's existing device starts a
  fanout, pages account rooms with bounded one-room discovery, claims one
  target-device KeyPackage per room, persists each claimed package with the
  encrypted fanout plan, prepares both add Commits, loses the first submit
  response after the server accepted it, restarts from stored prepared Commits,
  retries idempotently, completes both rooms from the ordered log, and then the
  target device claims and activates both Welcomes through the normal runtime
  sync tick. The first version of this proof exposed a cursor/MLS mismatch in
  the setup: Alice had already merged the setup Commit but the encrypted cursor
  still pointed at zero, so the worker tried to process an old epoch. The test
  now explicitly persists the setup cursors before starting fanout.
- The same-epoch runtime fanout proof closes the retry hole around prepared
  link adds. Alice prepares a later-device add, submit fails before reaching
  the server, Bob wins the epoch with a self-update, Alice processes Bob's
  ordered Commit and clears her losing pending Commit, then the fanout worker
  reuses the still-leased claimed KeyPackage from encrypted fanout state,
  prepares a fresh add at the new epoch, submits it, completes from the ordered
  log, and the target device activates its Welcome.
- The revocation checkpoint clones Charlie's client state before removal to
  model a stale/lost device. After Bob removes Charlie, that stale client can
  fetch and process the removal Commit, but the server rejects its old-epoch
  send, rejects a forged new-epoch send as inactive, withholds post-remove log
  entries, and OpenMLS rejects a leaked post-remove ciphertext.
- The durable device-status checkpoint adds the server-side revocation ledger
  that room MLS removal needs around it. Revoked devices cannot replenish or
  claim KeyPackages, cannot claim or activate pending Welcomes, cannot send
  application events, and cannot submit Commits. SQLite proves the status
  survives reopen.
- The same-epoch recovery checkpoint creates two real local pending Commits at
  epoch 1. Alice's add wins, Bob's add loses with `WrongEpoch`, Bob keeps local
  pending state until he observes Alice's ordered Commit, then `apply_log_entry`
  clears the loser, processes the winner, and lets Bob retry at epoch 2. The
  retry reuses the still-leased Dana KeyPackage because the rejected Commit did
  not consume it or release a Welcome.
- The broader same-epoch recovery checkpoint keeps that same branch under real
  OpenMLS for non-add operations. An update loser retries after an update
  winner, a remove loser retries after an update winner, and a device whose
  pending update lost because it was removed clears pending state, cannot retry,
  cannot send locally or through the server, and stops receiving future entries.
- The KeyPackage replenishment checkpoint uses real OpenMLS package bytes for
  the client boundary: exact duplicate upload retry is idempotent, conflicting
  duplicate upload is rejected, account claim exhaustion returns no packages,
  uploading a fresh package replenishes availability, and lease expiry makes the
  original package reclaimable. The client planner now takes server inventory,
  generates only the missing upload requests needed to reach a target, auto-ids
  packages from their MLS payload hash, persists pending upload requests in
  encrypted client state, and refuses over-cap targets. The runtime tick saves
  local OpenMLS state plus replayable pending uploads before upload so a
  server-visible KeyPackage is not missing its local private state after
  restart. Sim and SQLite prove the server cap counts available plus leased
  packages, accepted add Commits free consumed package space, and cap behavior
  survives reopen. The response-loss runtime test proved the earlier
  save-before-upload rule was incomplete by itself: after the server accepted
  one upload and the client crashed before local clear, restart retried the
  exact pending upload idempotently and did not generate extra local packages.
- The runtime sync checkpoint exposed a gap in the earlier crash proof: we had
  durable activation before server ack, but no durable marker telling the
  automated runtime loop to send that ack after restart. The fix adds
  `pending_welcome_acks` to encrypted client state and makes server ack retry
  safe.
- The history-policy checkpoint makes the v1 product decision executable:
  Alice's newly linked phone syncs from cursor zero, but the server only returns
  entries from the accepted add Commit forward. The phone decrypts the
  post-invite message and never receives Bob's pre-invite room-log message.
- Existing server tests mattered again here: the first version of the remote
  add proof accidentally used a direct room for a third-account add, and
  `DirectRoomThirdAccount` failed the scenario before it could become false
  confidence. The proof now uses a group room while direct-room limits remain
  covered separately.

## SQLite Follow-Up

The SQLite suite lives in
`crates/finitechat-store/tests/sqlite_scenarios.rs`.

Proven SQLite restart scenarios:

- `sqlite_create_dm_room_and_release_welcome_after_commit`
- `sqlite_key_package_payload_survives_reopen_and_claim`
- `sqlite_duplicate_key_package_upload_is_rejected_after_reopen`
- `sqlite_account_key_package_claim_survives_reopen`
- `sqlite_key_package_inventory_cap_survives_reopen_and_consumed_frees_space`
- `sqlite_revoked_device_status_survives_reopen_and_blocks_key_packages`
- `sqlite_claimed_welcome_payload_survives_reopen`
- `sqlite_revoked_device_blocks_welcome_activation_and_sends_after_reopen`
- `sqlite_add_commit_requires_staged_welcome_bytes_before_mutation`
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
- `sqlite_account_room_discovery_pages_after_reopen`
- `sqlite_duplicate_pending_device_add_is_rejected_before_side_effects`
- `sqlite_direct_room_create_or_get_and_third_account_rejection`
- `sqlite_oversized_application_payload_is_rejected_without_persisting_log`
- `sqlite_sync_events_returns_bounded_page_after_reopen`
- `sqlite_duplicate_message_id_is_typed_engine_error`
- `sqlite_link_payload_limit_is_rejected`
- `sqlite_idempotency_capacity_rejects_new_mutations_but_allows_replay`
- `sqlite_operation_fuzz_matches_in_memory_delivery_service`
- `sqlite_commit_crash_matrix_rolls_back_and_retry_converges`
- `sqlite_commit_epoch_unique_index_blocks_second_commit_row`

The SQLite crash matrix injects transaction rollbacks after log append, room
head update, removed membership update, added membership insert, KeyPackage
consumption, Welcome release, and idempotency record insert. It then reopens the
store, retries the same Commit, and verifies convergence to one log entry, one
epoch advance, correct membership intervals, consumed KeyPackages, released
Welcomes, and a replayable idempotency result.

The SQLite operation fuzzer applies the same deterministic sequence to the
in-memory reducer and SQLite store, then compares room, device, KeyPackage,
KeyPackage inventory, and Welcome state after every operation. It mixes
register/revoke, upload/claim, account claim, lease expiry, Welcome claim/ack,
app events, add/remove Commits, stale epochs, and exact idempotent retries. The
first version caught a real reducer/store drift: explicit claim of a leased
KeyPackage owned by a revoked device returned `KeyPackageUnavailable` in memory
but `DeviceRevoked` in SQLite. The store now matches the reducer ordering.

Push outbox rows are not implemented yet; when they land, this matrix should add
a failure point after outbox enqueue and assert exactly one durable wake record.

## Planned Activity Scenarios

The Pika typing-indicator behavior should become explicit Finite Chat protocol
coverage before the room server owns push fanout:

- `ephemeral_activity_never_enqueues_push`
- `ephemeral_activity_expires_without_transcript_entry`
- `ephemeral_activity_does_not_create_command_inbox_work`
- `ephemeral_activity_does_not_advance_room_sequence`
- `durable_sync_cursor_ignores_ephemeral_activity`
- `ephemeral_activity_rejects_non_member_device`
- `ephemeral_activity_rejects_pending_unacked_device`
- `ephemeral_activity_rejects_removed_or_revoked_device`
- `ephemeral_activity_payload_is_opaque_to_server`
- `ephemeral_activity_epoch_mismatch_drops_without_repair`
- `ephemeral_activity_routes_by_optional_conversation_id`
- `conversation_id_does_not_authorize_cross_room_activity`
- `activity_projection_keeps_devices_separate`
- `activity_projection_rolls_up_identity_for_normal_ui`
- `activity_projection_does_not_clear_sibling_device_activity`
- `activity_refresh_extends_matching_device_expiry`
- `activity_clear_removes_matching_device_kind_and_activity_id`
- `activity_clear_does_not_remove_unrelated_kind`
- `activity_clear_does_not_remove_different_activity_id`
- `stale_agent_activity_clear_does_not_hide_newer_run`
- `long_running_agent_activity_uses_command_or_run_id`
- `long_running_agent_activity_survives_refresh_without_push`
- `ephemeral_activity_expiry_is_bounded`
- `durable_chat_message_clears_matching_default_typing`
- `durable_command_result_clears_matching_working_activity`
- `dropped_ephemeral_clear_is_repaired_by_durable_terminal_event`
- `durable_terminal_clear_is_sender_scoped`
- `durable_terminal_clear_does_not_remove_different_activity_id`
- `reserved_activity_kinds_render_generically`
- `unknown_namespaced_activity_kind_is_preserved`
- `app_specific_activity_kind_does_not_trigger_generic_ui`
