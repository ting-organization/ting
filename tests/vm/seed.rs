//! A chain's seed, built as the seed of a chain is: an empty block, then
//! every case in its section's order, its transactions in order, packed into
//! blocks on the chain's genesis and mined. Each section starts a block.
//! Each block receipt must equal the one its case produced on its own, with
//! every check passed, and a fresh validator must replay the written bundle to
//! the same blocks and the same state.
//!
//! The bundles are `conformance/seed/<chain>.blocks`, each block framed with
//! a u32 length, which the node carries and every node of the chain starts
//! with, and `<chain>.json`, the manifest naming each block's section and each
//! transaction's case and step. The development seed is rebuilt by every run:
//! the test fails where it differs from the published one, and
//! `TING_WRITE_VECTORS=1` rewrites it. The production seed was built once, at
//! the launch, with `TING_SEED_CHAIN=production` (`network`), mined at the
//! production difficulty; from then on the chain's history is the rule, and
//! the published bundle only replays.
use crate::{
    across, budgets, calls, compositions, control, deployment,
    dsl::{import, invoke, memory, module},
    generated,
    harness::{self, context, deployed as case},
    heavy, host, isolation, memory, network, numeric, primitives, programs, reservations,
    state_histories, validation, wide,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::atomic::Ordering,
};
use ting_core::{
    consensus::{Ancestry, active_limits, valid_pow},
    hash::hex,
    protocol::{Block, Encode, Genesis, HEADER_LEN, Header, target_bytes},
    state::State,
    vm::{Context, Runtime},
};
use vm_model::{Ledger, cost, words};

/// A section of the seed: its name and its tests by name.
type Section = (&'static str, &'static [(&'static str, fn())]);
/// The seed's sections in order; the seed runs their tests and places their
/// cases in this order.
pub(crate) const SECTIONS: &[Section] = &[
    (
        "S1 primitives",
        &[
            ("primitives::binary_data_and_zero_compute_remain_valid_without_application_policy", primitives::binary_data_and_zero_compute_remain_valid_without_application_policy),
            ("primitives::blocks_of_one_to_nine_transactions", primitives::blocks_of_one_to_nine_transactions),
            ("primitives::the_same_transaction_twice_in_one_block", primitives::the_same_transaction_twice_in_one_block),
            ("primitives::a_call_of_an_identifier_nothing_is_deployed_under", primitives::a_call_of_an_identifier_nothing_is_deployed_under),
        ],
    ),
    (
        "S2 validation",
        &[
            ("validation::consensus_extensions_are_rejected_even_when_the_program_never_uses_them", validation::consensus_extensions_are_rejected_even_when_the_program_never_uses_them),
            ("validation::the_structural_bounds_at_their_edges", validation::the_structural_bounds_at_their_edges),
            ("validation::memory_declarations", validation::memory_declarations),
            ("validation::imports_are_the_twelve_host_functions_with_their_exact_signatures", validation::imports_are_the_twelve_host_functions_with_their_exact_signatures),
            ("validation::exports_are_invoke_and_the_memory_alone", validation::exports_are_invoke_and_the_memory_alone),
            ("validation::instructions_outside_the_profile_are_rejected", validation::instructions_outside_the_profile_are_rejected),
            ("validation::typing_rules_of_the_core_validation_algorithm", validation::typing_rules_of_the_core_validation_algorithm),
            ("validation::tables_and_element_segments", validation::tables_and_element_segments),
            ("validation::globals_and_data_segments", validation::globals_and_data_segments),
            ("validation::binary_encodings_are_minimal_complete_and_in_order", validation::binary_encodings_are_minimal_complete_and_in_order),
            ("validation::invalid_code_keeps_a_code_price_read_from_its_framing_alone", validation::invalid_code_keeps_a_code_price_read_from_its_framing_alone),
            ("validation::a_code_price_beyond_the_budget_is_refused_whole_before_validation", validation::a_code_price_beyond_the_budget_is_refused_whole_before_validation),
            ("validation::an_if_without_else_may_not_carry_a_result_even_in_dead_code", validation::an_if_without_else_may_not_carry_a_result_even_in_dead_code),
            ("validation::every_immediate_is_accepted_minimally_encoded_and_in_no_other_form", validation::every_immediate_is_accepted_minimally_encoded_and_in_no_other_form),
            ("validation::one_encoding_per_construct_no_empty_section_and_one_group_per_run", validation::one_encoding_per_construct_no_empty_section_and_one_group_per_run),
            ("validation::sections_are_complete_in_order_and_unique", validation::sections_are_complete_in_order_and_unique),
            ("validation::control_constructs_accept_exactly_the_bodies_core_validation_allows", validation::control_constructs_accept_exactly_the_bodies_core_validation_allows),
            ("validation::dead_code_is_typed_where_its_operands_are_known", validation::dead_code_is_typed_where_its_operands_are_known),
        ],
    ),
    (
        "S3 instructions",
        &[
            ("numeric::i32_binary_operators_over_edge_operands", numeric::i32_binary_operators_over_edge_operands),
            ("numeric::i64_binary_operators_over_edge_operands", numeric::i64_binary_operators_over_edge_operands),
            ("numeric::unary_operators_count_bits", numeric::unary_operators_count_bits),
            ("numeric::conversions_wrap_and_extend", numeric::conversions_wrap_and_extend),
            ("numeric::sign_extension_operators_follow_core_semantics", numeric::sign_extension_operators_follow_core_semantics),
            ("numeric::expressions_are_not_folded_around_overflow_and_signedness", numeric::expressions_are_not_folded_around_overflow_and_signedness),
            ("control::select_takes_the_first_operand_for_a_nonzero_condition", control::select_takes_the_first_operand_for_a_nonzero_condition),
            ("control::br_if_carries_the_block_result_when_taken", control::br_if_carries_the_block_result_when_taken),
            ("control::if_runs_one_arm_and_its_delimiters_cost_nothing", control::if_runs_one_arm_and_its_delimiters_cost_nothing),
            ("control::br_table_uses_the_default_for_indices_out_of_range", control::br_table_uses_the_default_for_indices_out_of_range),
            ("control::a_br_table_of_any_width_reaches_its_last_label", control::a_br_table_of_any_width_reaches_its_last_label),
            ("control::a_loop_branch_does_not_re_enter_the_loop_opener", control::a_loop_branch_does_not_re_enter_the_loop_opener),
            ("control::local_tee_keeps_the_value_on_the_stack", control::local_tee_keeps_the_value_on_the_stack),
            ("control::globals_start_fresh_on_every_invocation", control::globals_start_fresh_on_every_invocation),
            ("control::a_function_returns_explicitly_implicitly_or_through_its_label", control::a_function_returns_explicitly_implicitly_or_through_its_label),
            ("control::unreachable_traps_after_its_own_dispatch", control::unreachable_traps_after_its_own_dispatch),
            ("control::select_evaluates_both_operands", control::select_evaluates_both_operands),
            ("control::indirect_calls_check_the_entry_and_the_type_before_reserving", control::indirect_calls_check_the_entry_and_the_type_before_reserving),
            ("control::an_imported_function_in_the_table_pays_the_host_call_and_reserves_nothing", control::an_imported_function_in_the_table_pays_the_host_call_and_reserves_nothing),
            ("control::memory_fill_and_copy_charge_their_bytes_per_word", control::memory_fill_and_copy_charge_their_bytes_per_word),
            ("control::memory_copy_handles_overlap_in_both_directions", control::memory_copy_handles_overlap_in_both_directions),
            ("control::bulk_operations_at_the_end_of_memory", control::bulk_operations_at_the_end_of_memory),
            ("control::operands_are_evaluated_left_to_right", control::operands_are_evaluated_left_to_right),
            ("control::host_arguments_are_evaluated_left_to_right", control::host_arguments_are_evaluated_left_to_right),
            ("control::parameters_arrive_in_the_order_they_were_pushed", control::parameters_arrive_in_the_order_they_were_pushed),
            ("control::a_branch_carries_the_top_value_and_discards_the_rest", control::a_branch_carries_the_top_value_and_discards_the_rest),
            ("control::a_function_returns_its_top_value_and_discards_the_rest", control::a_function_returns_its_top_value_and_discards_the_rest),
            ("control::a_br_table_switch_falls_through_the_blocks_it_does_not_leave", control::a_br_table_switch_falls_through_the_blocks_it_does_not_leave),
            ("wide::add128_carries_into_the_high_half_and_wraps_at_two_to_the_128", wide::add128_carries_into_the_high_half_and_wraps_at_two_to_the_128),
            ("wide::sub128_borrows_from_the_high_half_and_wraps_below_zero", wide::sub128_borrows_from_the_high_half_and_wraps_below_zero),
            ("wide::mul_wide_u_keeps_the_whole_unsigned_product", wide::mul_wide_u_keeps_the_whole_unsigned_product),
            ("wide::mul_wide_s_keeps_the_whole_signed_product_and_never_traps", wide::mul_wide_s_keeps_the_whole_signed_product_and_never_traps),
            ("wide::every_pair_of_edges", wide::every_pair_of_edges),
            ("wide::seeded_operands", wide::seeded_operands),
            ("wide::the_low_half_is_pushed_first_and_the_high_half_is_on_top", wide::the_low_half_is_pushed_first_and_the_high_half_is_on_top),
            ("wide::they_are_priced_two_and_eight", wide::they_are_priced_two_and_eight),
            ("wide::two_results_take_two_slots_of_the_operand_height", wide::two_results_take_two_slots_of_the_operand_height),
            ("wide::operands_must_be_i64_and_both_results_must_be_consumed", wide::operands_must_be_i64_and_both_results_must_be_consumed),
            ("wide::each_is_fc_and_its_sub_opcode_without_immediates", wide::each_is_fc_and_its_sub_opcode_without_immediates),
        ],
    ),
    (
        "S4 memory and reservations",
        &[
            ("memory::loads_extend_and_read_little_endian_at_any_alignment", memory::loads_extend_and_read_little_endian_at_any_alignment),
            ("memory::loads_at_the_exact_end_succeed_and_one_byte_past_it_trap", memory::loads_at_the_exact_end_succeed_and_one_byte_past_it_trap),
            ("memory::stores_truncate_and_write_little_endian_at_any_alignment", memory::stores_truncate_and_write_little_endian_at_any_alignment),
            ("memory::memory_grow_pays_for_its_pages_and_reports_minus_one_beyond_the_maximum", memory::memory_grow_pays_for_its_pages_and_reports_minus_one_beyond_the_maximum),
            ("memory::grown_memory_is_zero_to_its_last_byte", memory::grown_memory_is_zero_to_its_last_byte),
            ("memory::loads_and_stores_are_priced_by_the_tier_memory_has_grown_into", memory::loads_and_stores_are_priced_by_the_tier_memory_has_grown_into),
            ("memory::loads_and_stores_follow_the_curve_above_2_mib", memory::loads_and_stores_follow_the_curve_above_2_mib),
            ("memory::every_span_above_2_mib_pays_as_a_store", memory::every_span_above_2_mib_pays_as_a_store),
            ("memory::a_declared_memory_sets_the_tier_from_the_first_instruction", memory::a_declared_memory_sets_the_tier_from_the_first_instruction),
            ("memory::data_segments_apply_in_order", memory::data_segments_apply_in_order),
            ("memory::a_large_declared_memory_is_paid_per_page", memory::a_large_declared_memory_is_paid_per_page),
            ("memory::memory_copy_over_every_small_overlap", memory::memory_copy_over_every_small_overlap),
            ("memory::memory_copy_and_fill_across_word_boundaries", memory::memory_copy_and_fill_across_word_boundaries),
            ("memory::a_zero_length_bulk_operation_still_needs_a_range_inside_memory", memory::a_zero_length_bulk_operation_still_needs_a_range_inside_memory),
            ("memory::every_load_and_store_at_every_edge_of_memory", memory::every_load_and_store_at_every_edge_of_memory),
            ("memory::loads_read_the_same_bytes_whatever_the_offset_split_and_alignment_hint", memory::loads_read_the_same_bytes_whatever_the_offset_split_and_alignment_hint),
            ("reservations::recursion_reserves_at_every_level", reservations::recursion_reserves_at_every_level),
            ("reservations::parameters_and_locals_are_reserved_together_and_atomically", reservations::parameters_and_locals_are_reserved_together_and_atomically),
            ("reservations::the_operand_height_is_reserved_as_the_validator_computes_it", reservations::the_operand_height_is_reserved_as_the_validator_computes_it),
            ("reservations::unbounded_recursion_ends_exactly_where_the_budget_does", reservations::unbounded_recursion_ends_exactly_where_the_budget_does),
            ("reservations::the_stack_pays_for_each_64_kib_its_mark_passes_once", reservations::the_stack_pays_for_each_64_kib_its_mark_passes_once),
            ("reservations::an_entry_function_beyond_64_kib_pays_its_stack_pages_on_entry", reservations::an_entry_function_beyond_64_kib_pays_its_stack_pages_on_entry),
            ("host::input_and_producer_getters_copy_whole_or_nothing_across_price_steps", host::input_and_producer_getters_copy_whole_or_nothing_across_price_steps),
            ("host::storage_get_snapshots_an_aliased_key_before_copying_its_value", host::storage_get_snapshots_an_aliased_key_before_copying_its_value),
            ("calls::deployment_receives_code_and_input_as_two_spans_rounded_together", calls::deployment_receives_code_and_input_as_two_spans_rounded_together),
            ("memory::every_affordable_memory_price_transition_has_a_byte_witness", memory::every_affordable_memory_price_transition_has_a_byte_witness),
        ],
    ),
    (
        "S5 host imports",
        &[
            ("host::code_hash_snapshots_aliased_targets_and_leaves_short_outputs_untouched", host::code_hash_snapshots_aliased_targets_and_leaves_short_outputs_untouched),
            ("host::spans_are_validated_before_argument_bytes_are_charged", host::spans_are_validated_before_argument_bytes_are_charged),
            ("host::input_reports_its_length_and_copies_only_when_it_fits", host::input_reports_its_length_and_copies_only_when_it_fits),
            ("host::a_getter_span_is_checked_in_full_even_for_an_empty_input", host::a_getter_span_is_checked_in_full_even_for_an_empty_input),
            ("host::a_zero_length_span_still_needs_a_memory", host::a_zero_length_span_still_needs_a_memory),
            ("host::context_selectors_report_the_block_the_transaction_and_the_budget", host::context_selectors_report_the_block_the_transaction_and_the_budget),
            ("host::producer_data_is_copied_only_when_it_fits", host::producer_data_is_copied_only_when_it_fits),
            ("host::producer_length_queries_move_no_bytes", host::producer_length_queries_move_no_bytes),
            ("host::host_copies_pay_a_span_each_above_2_mib", host::host_copies_pay_a_span_each_above_2_mib),
            ("host::storage_get_reports_the_length_and_copies_only_when_it_fits", host::storage_get_reports_the_length_and_copies_only_when_it_fits),
            ("host::a_length_query_of_a_large_record_pays_its_key_and_its_access_only", host::a_length_query_of_a_large_record_pays_its_key_and_its_access_only),
            ("host::a_record_read_debits_its_key_with_its_access", host::a_record_read_debits_its_key_with_its_access),
            ("host::storage_set_charges_the_value_per_word_and_growth_gross", host::storage_set_charges_the_value_per_word_and_growth_gross),
            ("host::storage_delete_pays_its_two_accesses_whether_or_not_the_record_exists", host::storage_delete_pays_its_two_accesses_whether_or_not_the_record_exists),
            ("host::storage_set_fails_atomically_when_access_or_growth_runs_out", host::storage_set_fails_atomically_when_access_or_growth_runs_out),
            ("host::an_empty_record_is_present_until_deleted", host::an_empty_record_is_present_until_deleted),
            ("host::code_hash_reads_another_programs_manifest", host::code_hash_reads_another_programs_manifest),
            ("host::return_revert_and_emit_charge_their_payload_and_validate_their_span", host::return_revert_and_emit_charge_their_payload_and_validate_their_span),
            ("host::a_copy_that_does_not_fit_stops_the_import_after_its_own_price", host::a_copy_that_does_not_fit_stops_the_import_after_its_own_price),
            ("host::call_result_stops_when_its_copy_does_not_fit", host::call_result_stops_when_its_copy_does_not_fit),
            ("host::a_record_lookup_that_does_not_fit_stops_the_import", host::a_record_lookup_that_does_not_fit_stops_the_import),
        ],
    ),
    (
        "S6 deployment and calls",
        &[
            ("deployment::a_deployment_at_each_budget_boundary", deployment::a_deployment_at_each_budget_boundary),
            ("deployment::the_same_transaction_again_collides_and_the_program_stays_callable", deployment::the_same_transaction_again_collides_and_the_program_stays_callable),
            ("deployment::an_initializer_that_traps_fails_the_deployment_and_installs_nothing", deployment::an_initializer_that_traps_fails_the_deployment_and_installs_nothing),
            ("deployment::an_initializer_out_of_compute_stops_exactly_at_its_limit", deployment::an_initializer_out_of_compute_stops_exactly_at_its_limit),
            ("deployment::an_initializers_input_and_result_are_charged_and_the_result_discarded", deployment::an_initializers_input_and_result_are_charged_and_the_result_discarded),
            ("deployment::the_identifier_depends_on_the_chain_the_transaction_and_the_ordinal_alone", deployment::the_identifier_depends_on_the_chain_the_transaction_and_the_ordinal_alone),
            ("calls::a_childs_charges_stay_whatever_becomes_of_its_effects", calls::a_childs_charges_stay_whatever_becomes_of_its_effects),
            ("calls::a_parent_failure_undoes_successful_child_writes_and_events_but_keeps_charges", calls::a_parent_failure_undoes_successful_child_writes_and_events_but_keeps_charges),
            ("calls::call_checks_its_flags_before_copying_and_refuses_an_excessive_budget", calls::call_checks_its_flags_before_copying_and_refuses_an_excessive_budget),
            ("calls::read_only_is_refused_before_the_spans_of_a_mutation_are_checked", calls::read_only_is_refused_before_the_spans_of_a_mutation_are_checked),
            ("calls::the_caller_identity_comes_from_the_vm", calls::the_caller_identity_comes_from_the_vm),
            ("calls::a_call_pays_its_spans_at_the_callers_memory_size", calls::a_call_pays_its_spans_at_the_callers_memory_size),
            ("calls::read_only_is_inherited_through_an_ordinary_call", calls::read_only_is_inherited_through_an_ordinary_call),
            ("calls::the_last_result_is_cleared_by_a_refused_call", calls::the_last_result_is_cleared_by_a_refused_call),
            ("calls::internal_deployments_advance_the_ordinal_only_when_begun", calls::internal_deployments_advance_the_ordinal_only_when_begun),
            ("calls::code_is_prepared_once_per_transaction", calls::code_is_prepared_once_per_transaction),
            ("calls::a_failed_first_call_leaves_its_code_prepared", calls::a_failed_first_call_leaves_its_code_prepared),
            ("calls::a_deployment_prepares_its_code_and_pays_validation_regardless", calls::a_deployment_prepares_its_code_and_pays_validation_regardless),
            ("calls::a_child_grows_memory_against_compute_not_against_a_shared_cap", calls::a_child_grows_memory_against_compute_not_against_a_shared_cap),
            ("calls::self_recursion_is_bounded_by_the_budget_each_level_passes_on", calls::self_recursion_is_bounded_by_the_budget_each_level_passes_on),
            ("calls::four_hundred_nested_programs_do_not_recurse_on_the_native_stack", calls::four_hundred_nested_programs_do_not_recurse_on_the_native_stack),
            ("calls::a_child_pays_for_receiving_its_input", calls::a_child_pays_for_receiving_its_input),
            ("calls::everything_left_less_a_reserve_passes_an_input_of_any_size", calls::everything_left_less_a_reserve_passes_an_input_of_any_size),
            ("calls::a_deployment_pays_its_spans_at_the_callers_memory_size", calls::a_deployment_pays_its_spans_at_the_callers_memory_size),
            ("calls::a_deployment_that_cannot_receive_its_code_takes_no_ordinal", calls::a_deployment_that_cannot_receive_its_code_takes_no_ordinal),
            ("calls::a_child_traps_or_runs_out_of_data_or_growth_and_its_parent_continues", calls::a_child_traps_or_runs_out_of_data_or_growth_and_its_parent_continues),
            ("calls::an_internal_deployment_reverts_traps_or_runs_out_of_access_or_growth", calls::an_internal_deployment_reverts_traps_or_runs_out_of_access_or_growth),
            ("isolation::every_call_runs_a_fresh_instance_even_within_one_transaction", isolation::every_call_runs_a_fresh_instance_even_within_one_transaction),
            ("isolation::reentrant_storage_is_shared_but_instances_and_checkpoints_are_separate", isolation::reentrant_storage_is_shared_but_instances_and_checkpoints_are_separate),
            ("isolation::sibling_and_grandchild_effects_rollback_to_their_own_checkpoints", isolation::sibling_and_grandchild_effects_rollback_to_their_own_checkpoints),
            ("isolation::identical_code_keeps_contract_storage_and_read_only_frames_separate", isolation::identical_code_keeps_contract_storage_and_read_only_frames_separate),
            ("calls::failed_initializers_undo_descendants_without_rewinding_ordinals_or_preparation", calls::failed_initializers_undo_descendants_without_rewinding_ordinals_or_preparation),
            ("calls::last_results_survive_reads_and_are_replaced_by_every_child_attempt", calls::last_results_survive_reads_and_are_replaced_by_every_child_attempt),
            ("isolation::events_keep_their_order_across_a_nested_call", isolation::events_keep_their_order_across_a_nested_call),
            ("isolation::a_child_with_nothing_to_spend_fails_before_it_starts", isolation::a_child_with_nothing_to_spend_fails_before_it_starts),
            ("isolation::an_initializer_that_reverts_fails_the_deployment_with_its_bytes", isolation::an_initializer_that_reverts_fails_the_deployment_with_its_bytes),
            ("calls::nested_deployment_output_boundaries_keep_charges_and_ordinals", calls::nested_deployment_output_boundaries_keep_charges_and_ordinals),
            ("calls::indirect_children_resume_with_results_and_live_operands", calls::indirect_children_resume_with_results_and_live_operands),
            ("calls::grown_parents_resume_at_their_own_prices_after_a_child", calls::grown_parents_resume_at_their_own_prices_after_a_child),
        ],
    ),
    (
        "S7 across blocks",
        &[
            ("state_histories::thousands_of_records_survive_churn_failed_writes_and_later_reads", state_histories::thousands_of_records_survive_churn_failed_writes_and_later_reads),
            ("programs::a_counter_persists_across_transactions", programs::a_counter_persists_across_transactions),
            ("programs::a_counter_persists_across_blocks", programs::a_counter_persists_across_blocks),
            ("programs::byte_processing_of_inputs_of_every_size", programs::byte_processing_of_inputs_of_every_size),
            ("across::the_same_call_in_later_blocks_gives_the_same_receipt", across::the_same_call_in_later_blocks_gives_the_same_receipt),
            ("across::programs_deployed_in_different_blocks_call_each_other", across::programs_deployed_in_different_blocks_call_each_other),
            ("across::a_program_deployed_by_a_program_is_called_in_a_later_block", across::a_program_deployed_by_a_program_is_called_in_a_later_block),
            ("across::a_record_written_in_one_block_is_read_and_deleted_in_later_ones", across::a_record_written_in_one_block_is_read_and_deleted_in_later_ones),
        ],
    ),
    (
        "S8 budgets",
        &[
            ("budgets::remaining_context_tracks_nested_consumption_and_unused_allowances_in_every_counter", budgets::remaining_context_tracks_nested_consumption_and_unused_allowances_in_every_counter),
            ("budgets::output_debits_are_atomic_at_every_memory_step_and_word_edge", budgets::output_debits_are_atomic_at_every_memory_step_and_word_edge),
            ("budgets::compute_runs_out_at_the_final_return_debit", budgets::compute_runs_out_at_the_final_return_debit),
            ("budgets::compute_runs_out_exactly_at_the_instruction_whose_price_does_not_fit", budgets::compute_runs_out_exactly_at_the_instruction_whose_price_does_not_fit),
            ("budgets::seventeen_left_before_a_load_priced_eighteen", budgets::seventeen_left_before_a_load_priced_eighteen),
            ("budgets::a_loop_runs_out_part_way_through_an_iteration", budgets::a_loop_runs_out_part_way_through_an_iteration),
            ("budgets::a_reservation_that_does_not_fit_is_refused_whole", budgets::a_reservation_that_does_not_fit_is_refused_whole),
            ("budgets::the_entry_debits_come_in_order_and_are_each_refused_whole", budgets::the_entry_debits_come_in_order_and_are_each_refused_whole),
            ("budgets::the_entry_reads_the_manifest_and_the_code_before_loading_anything", budgets::the_entry_reads_the_manifest_and_the_code_before_loading_anything),
            ("budgets::data_runs_out_at_the_result_bytes", budgets::data_runs_out_at_the_result_bytes),
            ("budgets::a_result_debit_is_one_atomic_debit_of_compute_and_data", budgets::a_result_debit_is_one_atomic_debit_of_compute_and_data),
            ("budgets::a_page_that_does_not_fit_fails_before_it_is_added", budgets::a_page_that_does_not_fit_fails_before_it_is_added),
            ("budgets::a_reverted_write_keeps_its_growth_accesses_and_data", budgets::a_reverted_write_keeps_its_growth_accesses_and_data),
        ],
    ),
    (
        "S9 generated programs",
        &[
            ("compositions::seeded_expressions_evaluate_as_rust_does_with_exact_dispatch", compositions::seeded_expressions_evaluate_as_rust_does_with_exact_dispatch),
            ("generated::seeded_programs_of_statements_run_as_their_model_says", generated::seeded_programs_of_statements_run_as_their_model_says),
        ],
    ),
    (
        "S10 heavy blocks",
        &[
            ("heavy::several_ordered_transactions_reach_each_exact_block_capacity", heavy::several_ordered_transactions_reach_each_exact_block_capacity),
            ("heavy::random_small_copies_in_a_large_memory_fill_the_compute", heavy::random_small_copies_in_a_large_memory_fill_the_compute),
            ("heavy::one_memory_growth_fills_the_compute", heavy::one_memory_growth_fills_the_compute),
            ("heavy::a_record_written_per_access_pair_fills_the_accesses", heavy::a_record_written_per_access_pair_fills_the_accesses),
            ("heavy::events_fill_the_data", heavy::events_fill_the_data),
            ("heavy::one_input_fills_the_data", heavy::one_input_fills_the_data),
            ("heavy::calls_nested_as_deep_as_the_accesses_allow", heavy::calls_nested_as_deep_as_the_accesses_allow),
            ("heavy::one_record_grows_over_several_blocks_to_about_a_mebibyte", heavy::one_record_grows_over_several_blocks_to_about_a_mebibyte),
            ("heavy::discarded_child_outputs_fill_data_even_when_the_block_is_small", heavy::discarded_child_outputs_fill_data_even_when_the_block_is_small),
        ],
    ),
];

/// Tests of the seed's files that run on runtimes of their own and give the
/// seed no case: module verdicts beyond what a block holds, rejection
/// reasons, byte mutations and envelope rules, which consensus vectors take
/// over.
const REFERENCE: &[&str] = &[
    "validation::a_rejection_names_its_reason",
    "validation::accepted_byte_mutations_execute_deterministically_within_their_budget",
    "budgets::a_transaction_whose_data_budget_cannot_hold_its_envelope_is_no_transaction",
];

/// The tests of a file the seed runs, read from its source: every `#[test]`.
fn tests_in(module: &str, source: &str) -> BTreeSet<String> {
    source
        .replace("\r\n", "\n")
        .split("#[test]\npub(crate) fn ")
        .skip(1)
        .map(|rest| format!("{module}::{}", &rest[..rest.find('(').unwrap()]))
        .collect()
}

#[test]
fn every_test_of_a_seed_file_is_in_the_seed() {
    let mut files = tests_in("primitives", include_str!("primitives.rs"));
    files.extend(tests_in("validation", include_str!("validation.rs")));
    files.extend(tests_in("numeric", include_str!("numeric.rs")));
    files.extend(tests_in("control", include_str!("control.rs")));
    files.extend(tests_in("wide", include_str!("wide.rs")));
    files.extend(tests_in("memory", include_str!("memory.rs")));
    files.extend(tests_in("reservations", include_str!("reservations.rs")));
    files.extend(tests_in("host", include_str!("host.rs")));
    files.extend(tests_in("deployment", include_str!("deployment.rs")));
    files.extend(tests_in("calls", include_str!("calls.rs")));
    files.extend(tests_in("isolation", include_str!("isolation.rs")));
    files.extend(tests_in("programs", include_str!("programs.rs")));
    files.extend(tests_in("across", include_str!("across.rs")));
    files.extend(tests_in(
        "state_histories",
        include_str!("state_histories.rs"),
    ));
    files.extend(tests_in("budgets", include_str!("budgets.rs")));
    files.extend(tests_in("compositions", include_str!("compositions.rs")));
    files.extend(tests_in("generated", include_str!("generated.rs")));
    files.extend(tests_in("heavy", include_str!("heavy.rs")));
    let mut listed: BTreeSet<String> = SECTIONS
        .iter()
        .flat_map(|(_, tests)| tests.iter().map(|(name, _)| name.to_string()))
        .collect();
    listed.extend(REFERENCE.iter().map(|name| name.to_string()));
    assert_eq!(listed, files);
}

/// The directory the development seed is published in.
fn bundle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/seed")
}
/// The directory for what the tests write beside the published files: the
/// capacity rejections and the coverage report, under `target/`, or under
/// `TING_CONFORMANCE_OUTPUT` when that is set.
pub(crate) fn artifact_dir(profile: &str) -> PathBuf {
    std::env::var_os("TING_CONFORMANCE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("seed"))
        .join(profile)
}
/// The manifest: the genesis and the chain, and for each block its height,
/// id, section, producer data and resources used, with each transaction's
/// case and step on a line of its own.
fn manifest_text(genesis: &Genesis, blocks: &[serde_json::Value]) -> String {
    let mut out = format!(
        "{{\n  \"genesis\": \"{}\",\n  \"chain\": \"{}\",\n  \"blocks\": [\n",
        hex(&genesis.encode()),
        hex(&genesis.chain_id())
    );
    for (i, block) in blocks.iter().enumerate() {
        out += &format!(
            "    {{\"height\": {}, \"id\": {}, \"section\": {}, \"producer\": {}, \"used\": {}, ",
            block["height"], block["id"], block["section"], block["producer"], block["used"]
        );
        // Cases left out where this block would have held them: only on a
        // chain whose capacity cannot hold them.
        if block["left_out"].as_array().is_some_and(|l| !l.is_empty()) {
            out += &format!("\"left_out\": {}, ", block["left_out"]);
        }
        out += "\"transactions\": [";
        let transactions = block["transactions"].as_array().unwrap();
        for (k, tx) in transactions.iter().enumerate() {
            out += &format!(
                "\n      [{}, {}]{}",
                tx["case"],
                tx["step"],
                if k + 1 < transactions.len() {
                    ","
                } else {
                    "\n    "
                }
            );
        }
        out += if i + 1 < blocks.len() {
            "]},\n"
        } else {
            "]}\n"
        };
    }
    out + "  ]\n}\n"
}

/// The producer data of the seed's first block, which holds no transaction.
const EMPTY_BLOCK: &[u8] = b"an empty block";

/// The rejected counterpart of an exact-fit block. Its counters, receipts
/// and state are also checked by execution with just that one extra unit of
/// room, so a second unrelated defect cannot masquerade as capacity coverage.
fn capacity_rejection(
    block: &Block,
    dimension: usize,
    code: &[u8],
    records: &BTreeMap<[u8; 32], Vec<u8>>,
) -> Block {
    use ting_core::{
        consensus::commitments,
        hash::hash,
        meter::Resources,
        protocol::{Payload, Receipt, Status, Transaction},
    };
    let g = network::genesis();
    let mut bad = block.clone();
    match dimension {
        0 => {
            let tx = bad.transactions.last_mut().unwrap();
            tx.budget.0[0] += 1;
            let receipt = bad.receipts.last_mut().unwrap();
            receipt.transaction = tx.id(g.chain_id());
            receipt.used.0[0] += 1;
            bad.header.used.0[0] += 1;
        }
        2 => {
            bad.producer.push(0);
            bad.header.producer_hash = hash("producer", &bad.producer);
            bad.header.used.0[2] += 1;
        }
        _ => {
            let target =
                if let Payload::Call { target, .. } = bad.transactions.last().unwrap().payload {
                    target
                } else {
                    unreachable!()
                };
            let tx = Transaction {
                budget: Resources(vm_model::BUDGET),
                payload: Payload::Call {
                    target: if dimension == 1 { [0x5a; 32] } else { target },
                    input: if dimension == 1 { vec![] } else { vec![4] },
                },
            };
            let used = if dimension == 1 {
                [
                    1 + vm_model::KEY,
                    1,
                    vm_model::TRANSACTION_BASE + vm_model::call_size(0),
                    0,
                ]
            } else {
                let prefix = "i32.const 256 i32.const 1 call $input drop i32.const 256 i32.load8_u local.set $mode local.get $mode i32.eqz if local.get $mode i32.const 4 i32.eq if";
                let grow = "i32.const 0 i32.const 32 i32.const 5 call $storage_set i32.const 0 i32.const 0 call $return";
                vm_model::call(
                    1,
                    crate::shape::shape(code).entry(),
                    Ledger {
                        dispatch: vm_model::cost(crate::gate::ON_CALL)
                            + vm_model::cost_at(
                                &format!("{prefix} {grow}"),
                                vm_model::Tier::pages(17),
                            ),
                        copies: vm_model::words(1) + vm_model::words(37),
                        reads: 1,
                        hashing: vm_model::leaf(5),
                        accesses: 2,
                        growth: 1,
                        ..Default::default()
                    },
                    0,
                    &[],
                )
            };
            let receipt = Receipt {
                transaction: tx.id(g.chain_id()),
                status: if dimension == 1 {
                    Status::Missing
                } else {
                    Status::Success
                },
                used: Resources(used),
                result: vec![],
                events: vec![],
            };
            bad.transactions.push(tx);
            bad.receipts.push(receipt);
            for (value, extra) in bad.header.used.0.iter_mut().zip(used) {
                *value += extra;
            }
            if dimension == 3 {
                let mut expected = records.clone();
                expected.insert(harness::storage_key(target, [0; 32]), vec![0; 5]);
                bad.header.state_root = declared_root(&expected);
            }
        }
    }
    (bad.header.transactions_root, bad.header.receipts_root) =
        commitments(&bad.transactions, &bad.receipts);
    bad.header.nonce = 0;
    mine(&mut bad.header);
    bad
}

/// Gives `header` the first nonce from its own on whose id meets its target,
/// as a plain count upwards would, on every processor core: the threads take
/// consecutive ranges in turn, each the next as soon as it is done with one,
/// and the lowest range with a find gives the nonce. The header is hashed once
/// up to the nonce, its last 16 bytes, and each attempt compresses only the
/// block that holds them.
///
/// The production seed takes hours of mining, so there the nonces found are
/// also kept beside the seed's other output, and a build that starts again
/// takes each one back after checking it.
fn mine(header: &mut Header) {
    use sha2::digest::generic_array::GenericArray;
    use std::sync::{Mutex, atomic::AtomicU64};
    const PREFIX: &[u8] = b"Ting/\x05\x00block";
    const MESSAGE: usize = PREFIX.len() + HEADER_LEN;
    const PADDED: usize = (MESSAGE + 9).div_ceil(64) * 64;
    const FIXED: usize = (MESSAGE - 16) / 64 * 64;
    const NONCE: usize = MESSAGE - 16 - FIXED;
    const RANGE: u128 = 1 << 16;
    let kept = network::production().then(|| artifact_dir(network::name()).join("nonces"));
    let key = hex(&ting_core::hash::hash("seed-nonce", &header.encode()));
    if let Some(path) = &kept {
        let known = fs::read_to_string(path).unwrap_or_default();
        let nonce = known
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find(|(k, _)| *k == key)
            .map(|(_, nonce)| nonce.parse::<u128>().unwrap());
        if let Some(nonce) = nonce {
            let mut kept = header.clone();
            kept.nonce = nonce;
            if valid_pow(&kept) {
                *header = kept;
                return;
            }
        }
    }
    let mut message = [0u8; PADDED];
    message[..PREFIX.len()].copy_from_slice(PREFIX);
    message[PREFIX.len()..MESSAGE].copy_from_slice(&header.encode());
    message[MESSAGE] = 0x80;
    message[PADDED - 8..].copy_from_slice(&(MESSAGE as u64 * 8).to_be_bytes());
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for block in message[..FIXED].as_chunks::<64>().0 {
        sha2::compress256(&mut state, &[*GenericArray::from_slice(block)]);
    }
    let target: [u32; 8] = {
        let bytes = target_bytes(&header.target);
        std::array::from_fn(|i| u32::from_be_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()))
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let start = header.nonce;
    // The next range to take, the lowest range with a find, and the finds. A
    // range below the lowest find is always searched to its end, since ranges
    // are taken in order; one above it is given up.
    let next = AtomicU64::new(0);
    let lowest = AtomicU64::new(u64::MAX);
    let finds = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut tail: [u8; 64] = message[FIXED..].try_into().unwrap();
                'ranges: loop {
                    let range = next.fetch_add(1, Ordering::Relaxed);
                    if range > lowest.load(Ordering::Relaxed) {
                        return;
                    }
                    let from = start + u128::from(range) * RANGE;
                    for offset in 0..RANGE {
                        if offset % 4096 == 0 && lowest.load(Ordering::Relaxed) < range {
                            continue 'ranges;
                        }
                        let nonce = from + offset;
                        tail[NONCE..NONCE + 16].copy_from_slice(&nonce.to_le_bytes());
                        let mut id = state;
                        sha2::compress256(&mut id, &[*GenericArray::from_slice(&tail)]);
                        if id <= target {
                            lowest.fetch_min(range, Ordering::Relaxed);
                            finds.lock().unwrap().push((range, nonce));
                            continue 'ranges;
                        }
                    }
                }
            });
        }
    });
    let (_, nonce) = finds.into_inner().unwrap().into_iter().min().unwrap();
    header.nonce = nonce;
    assert!(valid_pow(header));
    if let Some(path) = &kept {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut known = fs::read_to_string(path).unwrap_or_default();
        known += &format!("{key} {nonce}\n");
        fs::write(path, known).unwrap();
    }
}

/// The seed built from `cases`: an empty block, then the cases' blocks, mined
/// on the seed's genesis; each block's manifest entry; and the validator that
/// applied them. A block ends where its next transaction does not fit,
/// starts a section, asks for a block or other producer data, or follows a
/// case that asks for one.
///
/// On a chain other than the development chain, a case whose first
/// transaction fits no empty block of the chain is left out, and the block
/// where it would have started names it under `left_out`; the development
/// chain's capacity holds every case.
pub(crate) fn build(cases: &[harness::Collected]) -> (Vec<Block>, Vec<serde_json::Value>, Runtime) {
    let genesis = network::genesis();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&genesis));
    let mut history = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut manifest = Vec::new();
    let mut expected = BTreeMap::new();
    let mut next = cases
        .iter()
        .enumerate()
        .flat_map(|(c, case)| (0..case.transactions.len()).map(move |s| (c, s)))
        .peekable();
    let mut first = true;
    while first || next.peek().is_some() {
        let ancestry = Ancestry::of(&history);
        let limits = active_limits(&genesis, &history);
        let height = history.len() as u64 + 1;
        let timestamp = crate::chain_histories::timestamp(&genesis, &history);
        // The block carries the producer data the next case asks for, or none.
        let producer = if first {
            EMPTY_BLOCK.to_vec()
        } else {
            next.peek()
                .and_then(|&(c, _)| cases[c].producer.clone())
                .unwrap_or_default()
        };
        let block = Context {
            chain: genesis.chain_id(),
            height,
            timestamp,
            parent: ancestry.parent_id(&genesis),
            producer: producer.clone(),
        };
        let mut candidate = runtime
            .candidate(
                &genesis,
                &ancestry,
                limits,
                timestamp,
                [0; 4],
                producer.clone(),
            )
            .unwrap();
        let mut placed: Vec<(usize, usize)> = Vec::new();
        let mut left_out: Vec<String> = Vec::new();
        while let Some(&(c, s)) = next.peek().filter(|_| !first) {
            let case = &cases[c];
            if let Some(&(c0, _)) = placed.first() {
                let ends = case.section != cases[c0].section
                    || case.new_block[s]
                    || (s == 0 && c > 0 && cases[c - 1].closes);
                if ends {
                    break;
                }
            }
            if case.producer.as_ref().is_some_and(|p| *p != producer) {
                break;
            }
            let Some(receipt) = candidate.add(&case.transactions[s]).unwrap() else {
                if placed.is_empty() && s == 0 && network::production() {
                    left_out.push(case.label.clone());
                    while next.peek().is_some_and(|&(c2, _)| c2 == c) {
                        next.next();
                    }
                    continue;
                }
                break;
            };
            // A case whose expectations depend on the run is checked in the
            // block it landed in; every other must give its own receipt.
            match &case.check {
                Some(check) => check(s, &block, receipt),
                None => assert_eq!(
                    hex(&receipt.encode()),
                    case.receipts[s],
                    "{}, transaction {s}: its receipt in block {height} differs from its own run",
                    case.label
                ),
            }
            placed.push((c, s));
            for (key, value) in &case.changes[s] {
                match value {
                    Some(bytes) => {
                        expected.insert(*key, bytes.clone());
                    }
                    None => {
                        expected.remove(key);
                    }
                }
            }
            next.next();
        }
        if placed.is_empty() && !first {
            let Some(&(c, s)) = next.peek() else {
                // Only left-out cases remained.
                break;
            };
            panic!(
                "{}, transaction {s}: it fits no empty block",
                cases[c].label
            );
        }
        first = false;
        let mut block = candidate.finish().unwrap();
        assert_eq!(
            block.header.target,
            crate::chain_histories::target(&genesis, &history),
            "block {height}: target differs from independent schedule arithmetic"
        );
        assert_eq!(
            block.header.state_root,
            declared_root(&expected),
            "block {height}: state root differs from the declared map's independent tree"
        );
        let mining = std::time::Instant::now();
        mine(&mut block.header);
        if network::production() {
            eprintln!(
                "block {height}: {} transactions, mined in {:.1} s",
                block.transactions.len(),
                mining.elapsed().as_secs_f64()
            );
        }
        let mut rejection = None;
        if let Some(&(c, 6)) = placed.last()
            && let Some((_, dimension)) = cases[c].sources.iter().find(|(name, _)| {
                name == "heavy::several_ordered_transactions_reach_each_exact_block_capacity"
            })
        {
            let dimension = *dimension;
            assert_eq!(
                placed.len(),
                6,
                "the exact block must hold all five mixed calls and its filler"
            );
            assert_eq!(block.header.used.0[dimension], limits.0[dimension]);
            assert_eq!(block.transactions[0], block.transactions[1]);
            assert_eq!(block.transactions[0], block.transactions[4]);
            let code = if let ting_core::protocol::Payload::Deploy { code, .. } =
                &cases[c].transactions[0].payload
            {
                code
            } else {
                unreachable!()
            };
            let bad = capacity_rejection(&block, dimension, code, &expected);
            assert_eq!(bad.header.used.0[dimension], limits.0[dimension] + 1);
            let mut relaxed = limits;
            relaxed.0[dimension] += 1;
            let mut check = runtime
                .candidate(
                    &genesis,
                    &ancestry,
                    relaxed,
                    bad.header.timestamp,
                    bad.header.votes,
                    bad.producer.clone(),
                )
                .unwrap();
            for tx in &bad.transactions {
                assert!(check.add(tx).unwrap().is_some());
            }
            let predicted = check.finish().unwrap();
            assert_eq!(predicted.receipts, bad.receipts);
            assert_eq!(predicted.header.used, bad.header.used);
            assert_eq!(predicted.header.state_root, bad.header.state_root);
            let before = runtime.state_root();
            let error = runtime
                .validate_and_apply_block_at(&genesis, &ancestry, &bad, limits)
                .unwrap_err();
            assert_eq!(error.0, "block resource limits");
            assert_eq!(runtime.state_root(), before);
            let vector = json!({"genesis":hex(&genesis.encode()),"parent":hex(&block.header.parent),
                "height":height,"dimension":dimension,"valid":false,"block":hex(&bad.encode())});
            let dir = artifact_dir(network::name()).join("rejections");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("capacity-{dimension}.json")),
                serde_json::to_vec_pretty(&vector).unwrap(),
            )
            .unwrap();
            rejection = Some(
                json!({"dimension":dimension,"id":hex(&bad.header.id()),"file":format!("rejections/capacity-{dimension}.json")}),
            );
        }
        runtime
            .validate_and_apply_block_at(&genesis, &ancestry, &block, limits)
            .unwrap();
        manifest.push(json!({
            "height": height,
            "id": hex(&block.header.id()),
            "section": placed.first().map(|&(c, _)| cases[c].section.clone()),
            "producer": hex(&producer),
            "used": block.header.used.0,
            "state_root": hex(&block.header.state_root),
            "transactions_root": hex(&block.header.transactions_root),
            "receipts_root": hex(&block.header.receipts_root),
            "live_records": expected.len(),
            "capacity_rejection": rejection,
            "left_out": left_out,
            "transactions": placed.iter().zip(&block.receipts).map(|(&(c, s), receipt)| json!({
                "case": cases[c].label,
                "sources": cases[c].sources,
                "step": s,
                "id": hex(&cases[c].transactions[s].id(genesis.chain_id())),
                "receipt": hex(&receipt.encode()),
            })).collect::<Vec<_>>(),
        }));
        history.push(block.header.clone());
        blocks.push(block);
    }
    (blocks, manifest, runtime)
}

/// The blocks framed with their lengths, as the bundle holds them.
pub(crate) fn bundle(blocks: &[Block]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for block in blocks {
        let encoded = block.encode();
        bytes.extend((encoded.len() as u32).to_le_bytes());
        bytes.extend(encoded);
    }
    bytes
}

/// The blocks of a written bundle of the seed's chain, applied by a fresh
/// validator from an empty state; the validator after the last.
pub(crate) fn replay(bytes: &[u8]) -> (Vec<Block>, Runtime) {
    replay_on(&network::genesis(), bytes)
}
/// The same on the chain of `genesis`.
pub(crate) fn replay_on(genesis: &Genesis, bytes: &[u8]) -> (Vec<Block>, Runtime) {
    let genesis = genesis.clone();
    let replay = Runtime::new(State::default(), Context::from_genesis(&genesis));
    let mut blocks: Vec<Block> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let block = Block::decode(&bytes[at + 4..at + 4 + len], len).unwrap();
        at += 4 + len;
        let history: Vec<_> = blocks.iter().map(|b| b.header.clone()).collect();
        let ancestry = Ancestry::of(&history);
        let limits = active_limits(&genesis, &history);
        replay
            .validate_and_apply_block_at(&genesis, &ancestry, &block, limits)
            .unwrap_or_else(|e| panic!("block {} does not replay: {e}", block.header.height));
        blocks.push(block);
    }
    (blocks, replay)
}

/// The seed of the cases' chain (`network`): every case of every section, run
/// with every check, placed once and built into blocks, with each block's
/// manifest entry and the validator that applied them.
pub(crate) fn seed() -> (Vec<Block>, Vec<serde_json::Value>, Runtime) {
    let (mut cases, mut empty) = (Vec::new(), Vec::new());
    for (section, tests) in SECTIONS {
        for (name, test) in tests.iter() {
            let mut collected = harness::collect(test);
            if collected.is_empty() {
                empty.push(*name);
            }
            for (variant, case) in collected.iter_mut().enumerate() {
                case.section = section.to_string();
                case.sources.push((name.to_string(), variant));
            }
            cases.extend(collected);
        }
    }
    assert!(
        empty.is_empty(),
        "tests that give the seed no case, for the reference tests: {empty:?}"
    );
    // A case run twice is one case on the chain: its transactions, and so
    // their identifiers, would be the same.
    let mut seen = BTreeMap::new();
    let mut unique: Vec<harness::Collected> = Vec::new();
    for mut case in cases {
        let key = (
            case.transactions
                .iter()
                .map(Encode::encode)
                .collect::<Vec<_>>(),
            case.new_block.clone(),
            case.closes,
            case.producer.clone(),
        );
        if let Some(&index) = seen.get(&key) {
            let old: &mut harness::Collected = &mut unique[index];
            assert_eq!(old.receipts, case.receipts, "duplicate fixtures disagree");
            assert_eq!(
                old.changes, case.changes,
                "duplicate state expectations disagree"
            );
            old.sources.append(&mut case.sources);
        } else {
            seen.insert(key, unique.len());
            unique.push(case);
        }
    }
    assert!(!unique.is_empty());
    build(&unique)
}

/// A separate, specification-shaped compact tree over the declared records.
/// No State, runtime roots, incremental nodes or consensus hash helpers are
/// used to derive it. Hash each value once, then partition ordered key slices.
pub(crate) fn declared_root(records: &BTreeMap<[u8; 32], Vec<u8>>) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    fn h(tag: &str, bytes: &[u8]) -> [u8; 32] {
        let mut input = b"Ting/".to_vec();
        input.extend((tag.len() as u16).to_le_bytes());
        input.extend(tag.as_bytes());
        input.extend(bytes);
        Sha256::digest(input).into()
    }
    fn walk(leaves: &[([u8; 32], [u8; 32])], depth: u16) -> [u8; 32] {
        match leaves {
            [] => h("state-empty", &[]),
            [(_, value)] => *value,
            _ => {
                assert!(depth < 256);
                let split = leaves.partition_point(|(key, _)| {
                    key[(depth / 8) as usize] & (0x80 >> (depth % 8)) == 0
                });
                let mut bytes = depth.to_le_bytes().to_vec();
                bytes.extend(walk(&leaves[..split], depth + 1));
                bytes.extend(walk(&leaves[split..], depth + 1));
                h("state-node", &bytes)
            }
        }
    }
    let leaves: Vec<_> = records
        .iter()
        .map(|(key, value)| {
            let mut bytes = key.to_vec();
            bytes.extend((value.len() as u32).to_le_bytes());
            bytes.extend(value);
            (*key, h("state-leaf", &bytes))
        })
        .collect();
    walk(&leaves, 0)
}

#[test]
fn the_development_seed_reproduces_every_case_and_replays() {
    if network::production() {
        // Built for the production chain, the cases make its seed instead.
        return;
    }
    let (blocks, manifest, runtime) = seed();
    let bytes = bundle(&blocks);
    let (replayed, validator) = replay(&bytes);
    assert_eq!(
        replayed.iter().map(|b| b.header.id()).collect::<Vec<_>>(),
        blocks.iter().map(|b| b.header.id()).collect::<Vec<_>>()
    );
    assert_eq!(validator.state_root(), runtime.state_root());
    // Check every manifest occurrence against the final bundle, including
    // dynamic block-context results and duplicate fixture aliases.
    assert_eq!(blocks.len(), manifest.len());
    let genesis = Genesis::development();
    let mut times: Vec<_> = blocks[1..12].iter().map(|b| b.header.timestamp).collect();
    times.sort_unstable();
    assert_eq!(blocks[12].header.timestamp, times[5] + 1);
    assert!(blocks[12].header.timestamp < blocks[11].header.timestamp);
    assert!(
        blocks
            .iter()
            .any(|b| b.header.target != genesis.initial_target)
    );
    assert_eq!(
        manifest
            .iter()
            .filter(|b| !b["capacity_rejection"].is_null())
            .count(),
        4
    );
    for (block, entry) in blocks.iter().zip(&manifest) {
        assert_eq!(entry["height"], block.header.height);
        assert_eq!(entry["id"], hex(&block.header.id()));
        let transactions = entry["transactions"].as_array().unwrap();
        assert_eq!(transactions.len(), block.transactions.len());
        assert_eq!(transactions.len(), block.receipts.len());
        for ((tx, receipt), stated) in block
            .transactions
            .iter()
            .zip(&block.receipts)
            .zip(transactions)
        {
            assert_eq!(stated["id"], hex(&tx.id(genesis.chain_id())));
            assert_eq!(stated["receipt"], hex(&receipt.encode()));
            assert!(!stated["sources"].as_array().unwrap().is_empty());
        }
    }
    // Publish only after all expectations and the independent replay pass:
    // the bundle and the manifest, as published.
    let dir = bundle_dir();
    let text = manifest_text(&genesis, &manifest);
    if std::env::var_os("TING_WRITE_VECTORS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("development.blocks"), &bytes).unwrap();
        fs::write(dir.join("development.json"), &text).unwrap();
    }
    let published = fs::read(dir.join("development.blocks")).unwrap_or_default();
    assert!(
        published == bytes,
        "conformance/seed/development.blocks differs from the seed; after a deliberate \
         change, rewrite it with TING_WRITE_VECTORS=1"
    );
    let published = fs::read_to_string(dir.join("development.json"))
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        published == text,
        "conformance/seed/development.json differs from the seed; after a deliberate \
         change, rewrite it with TING_WRITE_VECTORS=1"
    );

    // The base of the consensus vectors is the seed's beginning.
    let base = crate::consensus::base();
    assert_eq!(
        base.iter().map(|b| b.header.id()).collect::<Vec<_>>(),
        blocks[..base.len()]
            .iter()
            .map(|b| b.header.id())
            .collect::<Vec<_>>(),
        "the base of the block vectors is the development seed's first section"
    );
}

/// The cases the production chain's capacity cannot hold, left out of its
/// seed: modules with branch tables of 131,072 targets and more, whose code
/// alone grows the state by more than a production block's 131,072 G. The
/// development seed holds them.
const PRODUCTION_LEFT_OUT: &[&str] = &[
    "br_table of 131072 targets, index 0",
    "br_table of 131072 targets, index 131071",
    "br_table of 131072 targets, index 131072",
    "br_table of 131073 targets, index 0",
    "br_table of 131073 targets, index 131072",
    "br_table of 131073 targets, index 131073",
];

/// The cases' labels each block of a manifest names as left out, in order.
fn left_out(manifest: &[serde_json::Value]) -> Vec<String> {
    manifest
        .iter()
        .flat_map(|block| block["left_out"].as_array().cloned().unwrap_or_default())
        .map(|label| label.as_str().unwrap().to_string())
        .collect()
}

/// The production seed, built once at the launch: on the production genesis,
/// mined at its difficulty, which takes hours on every core of a laptop.
///
/// ```text
/// TING_SEED_CHAIN=production TING_WRITE_VECTORS=1 ///     cargo test --release --test vm -- --ignored the_production_seed_is_built_at_launch
/// ```
#[test]
#[ignore = "mines the production seed at the production difficulty, once, at the launch"]
fn the_production_seed_is_built_at_launch() {
    assert!(network::production(), "run with TING_SEED_CHAIN=production");
    let genesis = network::genesis();
    let (blocks, manifest, runtime) = seed();
    let bytes = bundle(&blocks);
    let (replayed, validator) = replay(&bytes);
    assert_eq!(
        replayed.iter().map(|b| b.header.id()).collect::<Vec<_>>(),
        blocks.iter().map(|b| b.header.id()).collect::<Vec<_>>()
    );
    assert_eq!(validator.state_root(), runtime.state_root());
    assert_eq!(left_out(&manifest), PRODUCTION_LEFT_OUT);
    let text = manifest_text(&genesis, &manifest);
    let dir = bundle_dir();
    if std::env::var_os("TING_WRITE_VECTORS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("production.blocks"), &bytes).unwrap();
        fs::write(dir.join("production.json"), &text).unwrap();
    }
    assert!(fs::read(dir.join("production.blocks")).unwrap_or_default() == bytes);
}

/// The published production seed replays on the production genesis to the
/// blocks its manifest names. It is the chain's history, which every change
/// to the VM must keep valid.
#[test]
fn the_published_production_seed_replays() {
    let genesis = Genesis::production(network::PRODUCTION_LAUNCH);
    let dir = bundle_dir();
    let bytes =
        fs::read(dir.join("production.blocks")).expect("conformance/seed/production.blocks");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.join("production.json")).expect("conformance/seed/production.json"),
    )
    .unwrap();
    assert_eq!(manifest["genesis"], hex(&genesis.encode()));
    assert_eq!(manifest["chain"], hex(&genesis.chain_id()));
    let (blocks, _) = replay_on(&genesis, &bytes);
    let stated = manifest["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), stated.len());
    for (block, entry) in blocks.iter().zip(stated) {
        assert_eq!(entry["height"], block.header.height);
        assert_eq!(entry["id"], hex(&block.header.id()));
        assert_eq!(
            entry["transactions"].as_array().unwrap().len(),
            block.transactions.len()
        );
    }
    assert_eq!(left_out(stated), PRODUCTION_LEFT_OUT);
}

/// A case that reads its block's height returns one height on its own and
/// another in the seed: the seed names it instead of taking either receipt.
#[test]
#[should_panic(expected = "differs from its own run")]
fn a_case_whose_receipt_depends_on_its_block_is_named() {
    let body = "i32.const 4 i32.const 0 i32.const 8 call $context drop i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("context"),
        &import("return"),
        &memory(1, 1),
        &invoke(body),
    ]);
    let cases = harness::collect(|| {
        case(&text)
            .call()
            .result(context().height.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(8),
                ..Default::default()
            })
            .run();
    });
    build(&cases);
}

/// The same case stating the height as its block's is checked in the block
/// it lands in.
#[test]
fn a_case_stating_its_block_values_is_checked_in_its_block() {
    let body = "i32.const 4 i32.const 0 i32.const 8 call $context drop i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("context"),
        &import("return"),
        &memory(1, 1),
        &invoke(body),
    ]);
    let cases = harness::collect(|| {
        case(&text)
            .call()
            .result_from(|f| f.block().height.to_le_bytes().to_vec())
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(8),
                ..Default::default()
            })
            .run();
    });
    let (blocks, manifest, _) = build(&cases);
    assert_eq!(blocks.len(), 2, "the empty block and the case's");
    let packed = &blocks[1].receipts[1];
    assert_eq!(packed.result, 2u64.to_le_bytes());
    assert_ne!(hex(&packed.encode()), cases[0].receipts[1]);
    assert_eq!(
        manifest[1]["transactions"][1]["receipt"],
        hex(&packed.encode()),
        "the manifest must record the checked packed receipt, not its isolated counterpart"
    );
}
