//! 代管控制事实的命名约束违反行。

use super::support::Case;

const fn check(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23514", sql }
}

const fn unique(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23505", sql }
}

pub const CASES: &[Case] = &[
    check(
        "consignment_custody_controls",
        "c4_control_id_ck",
        "UPDATE public.consignment_custody_controls SET id='00000000-0000-0000-0000-000000000000' WHERE id='a0000000-0000-4000-8000-000000000001';",
    ),
    check(
        "consignment_custody_controls",
        "c4_control_kind_ck",
        "UPDATE public.consignment_custody_controls SET kind='bogus' WHERE id='a0000000-0000-4000-8000-000000000001';",
    ),
    check(
        "consignment_custody_controls",
        "c4_control_actor_ck",
        "UPDATE public.consignment_custody_controls SET actor_ref='' WHERE id='a0000000-0000-4000-8000-000000000001';",
    ),
    check(
        "consignment_custody_controls",
        "c4_control_version_ck",
        "UPDATE public.consignment_custody_controls SET version=0 WHERE id='a0000000-0000-4000-8000-000000000001';",
    ),
    check(
        "consignment_custody_controls",
        "c4_control_time_ck",
        "UPDATE public.consignment_custody_controls SET occurred_at='infinity' WHERE id='a0000000-0000-4000-8000-000000000001';",
    ),
    unique(
        "consignment_custody_controls",
        "c4_control_version_uq",
        "UPDATE public.consignment_custody_controls SET version=2 WHERE id='a2000000-0000-4000-8000-000000000001';",
    ),
    unique(
        "consignment_custody_controls",
        "c4_control_predecessor_uq",
        "INSERT INTO public.consignment_custody_controls
            (id, position_id, kind, predecessor_id, command_id, actor_ref, version, occurred_at)
         SELECT 'a2000000-0000-4000-8000-000000000099', position_id, kind, predecessor_id,
                command_id, actor_ref, 4, occurred_at
         FROM public.consignment_custody_controls WHERE id='a2000000-0000-4000-8000-000000000001';",
    ),
];
