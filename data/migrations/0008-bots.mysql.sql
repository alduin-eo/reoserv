-- Autonomous player bots (Alduin TEST). Bots are real accounts flagged with is_bot so they can be
-- excluded from human statistics and refused real-money features (Alduin wallet).
ALTER TABLE `accounts` ADD COLUMN `is_bot` TINYINT NOT NULL DEFAULT 0;

CREATE TABLE
    IF NOT EXISTS `bot_accounts` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `account_name` VARCHAR(16) NOT NULL,
        `password` VARCHAR(32) NOT NULL,
        `character_name` VARCHAR(16),
        `personality` VARCHAR(16) NOT NULL,
        `mind` TEXT,
        `enabled` TINYINT NOT NULL DEFAULT 1,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        CONSTRAINT `bot_accounts_account_name_unique` UNIQUE (`account_name`),
        PRIMARY KEY (`id`)
    );

-- HGM $bots commands queue (written by reoserv, consumed by the bot manager service)
CREATE TABLE
    IF NOT EXISTS `bot_control` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `command` VARCHAR(16) NOT NULL,
        `arg` VARCHAR(64),
        `issued_by` VARCHAR(16),
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `processed_at` DATETIME,
        `result` TEXT,
        PRIMARY KEY (`id`)
    );

-- Live status published by the bot manager (read by $bots status/list/inspect)
CREATE TABLE
    IF NOT EXISTS `bot_status` (
        `character_name` VARCHAR(16) NOT NULL,
        `account_name` VARCHAR(16) NOT NULL,
        `personality` VARCHAR(16) NOT NULL,
        `level` INTEGER NOT NULL DEFAULT 0,
        `map` INTEGER NOT NULL DEFAULT 0,
        `x` INTEGER NOT NULL DEFAULT 0,
        `y` INTEGER NOT NULL DEFAULT 0,
        `activity` VARCHAR(64),
        `goal` VARCHAR(255),
        `online` TINYINT NOT NULL DEFAULT 0,
        `updated_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`character_name`)
    );

CREATE TABLE
    IF NOT EXISTS `bot_manager` (
        `id` INTEGER NOT NULL,
        `desired` INTEGER NOT NULL DEFAULT 0,
        `paused` TINYINT NOT NULL DEFAULT 0,
        `heartbeat` DATETIME,
        `summary` TEXT,
        PRIMARY KEY (`id`)
    );
