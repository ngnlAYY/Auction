-- FinalFast 仍是可降低上限的活动授权；同步数据库矩阵与领域转换校验。
ALTER TABLE public.bid_proxy_authorization_transitions
    DROP CONSTRAINT bid_proxy_authorization_transitions_matrix_check,
    ADD CONSTRAINT bid_proxy_authorization_transitions_matrix_check CHECK (
        (
            transition_kind = 'created'
            AND transition_number = 1
            AND old_maximum_amount IS NULL
            AND old_mode IS NULL
            AND old_status IS NULL
            AND new_status = 'active'
            AND reason IS NULL
            AND resulting_state_version = 1
        )
        OR (
            transition_kind = 'lowered'
            AND old_maximum_amount IS NOT NULL
            AND new_maximum_amount < old_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND old_status = new_status
            AND (old_status = 'active' OR old_mode = 'fast')
            AND reason IS NULL
        )
        OR (
            transition_kind = 'final_fast'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = 'slow'
            AND new_mode = 'fast'
            AND old_status = 'active'
            AND new_status = 'final_fast'
            AND reason IS NULL
        )
        OR (
            transition_kind = 'exhausted'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'exhausted'
            AND reason IN ('strictly_defeated', 'ranked_below')
        )
        OR (
            transition_kind = 'frozen'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'frozen'
            AND reason = 'eligibility_lost'
        )
        OR (
            transition_kind = 'closed'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'closed'
            AND reason = 'competition_closed'
        )
    );
