CREATE TABLE
    IF NOT EXISTS `character_transaction` (
        `id` INTEGER PRIMARY KEY,
        `character_id` INTEGER NOT NULL,
        `action_id` INTEGER NOT NULL,
        `amount` INTEGER NOT NULL,
        `settled_amount` INTEGER NOT NULL DEFAULT 0,
        `wallet_address` TEXT NOT NULL,
        `status_id` INTEGER NOT NULL DEFAULT 0,
        `created_at` INTEGER NOT NULL,
        `resolved_at` INTEGER,
        `resolved_by` INTEGER,
        `notified` INTEGER NOT NULL DEFAULT 0,
        FOREIGN KEY (`character_id`) REFERENCES `characters` (`id`)
    );

CREATE INDEX IF NOT EXISTS `idx_character_transaction_character_id_status`
    ON `character_transaction` (`character_id`, `status_id`);

CREATE INDEX IF NOT EXISTS `idx_character_transaction_status_notified`
    ON `character_transaction` (`status_id`, `notified`);

CREATE INDEX IF NOT EXISTS `idx_character_transaction_character_id_action_status`
    ON `character_transaction` (`character_id`, `action_id`, `status_id`);
