-- Final phase 继续复用周轮次租约，不建立第二套内存定时器；命令账本外键允许同一事务内后写命令。
ALTER TABLE public.scheduled_rounds
    ADD COLUMN final_command_id uuid,
    ADD COLUMN final_completed_at timestamptz,
    ADD CONSTRAINT scheduled_rounds_final_command_key
        UNIQUE (final_command_id) DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT scheduled_rounds_final_command_fkey
        FOREIGN KEY (final_command_id)
        REFERENCES public.auction_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT scheduled_rounds_final_shape_check CHECK (
        (final_completed_at IS NULL AND final_command_id IS NULL)
        OR (
            final_completed_at IS NOT NULL
            AND final_command_id IS NOT NULL
            AND open_completed_at IS NOT NULL
        )
    );

CREATE INDEX scheduled_rounds_final_worker_due_idx
    ON public.scheduled_rounds (lease_until, open_completed_at, id)
    WHERE status = 'active' AND final_completed_at IS NULL;
