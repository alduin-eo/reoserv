-- reoserv: skip-if-table-exists=character_transaction

CREATE TABLE IF NOT EXISTS `character_transaction` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `character_id` INTEGER NOT NULL,
    `action` VARCHAR(10) NOT NULL CHECK(`action` IN ('deposit', 'withdraw')),
    `amount` INTEGER NOT NULL,
    `settled_amount` INTEGER NOT NULL DEFAULT 0,
    `wallet_address` VARCHAR(255) NOT NULL,
    `status` VARCHAR(10) NOT NULL DEFAULT 'pending' CHECK(`status` IN ('pending', 'approved', 'cancelled')),
    `created_at` INTEGER NOT NULL,
    `resolved_at` INTEGER,
    `resolved_by` INTEGER,
    `notified` INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (`character_id`) REFERENCES `characters` (`id`) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS `idx_character_transaction_character_id_status` ON `character_transaction` (`character_id`, `status`);
CREATE INDEX IF NOT EXISTS `idx_character_transaction_status_notified` ON `character_transaction` (`status`, `notified`);
CREATE INDEX IF NOT EXISTS `idx_character_transaction_character_id_action_status` ON `character_transaction` (`character_id`, `action`, `status`);
