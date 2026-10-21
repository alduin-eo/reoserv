-- Admin control panel and player reports (Alduin). The website queues admin_requests; the game server
-- executes them with its own character/inventory/ban code and records the outcome.
CREATE TABLE
    IF NOT EXISTS `admin_requests` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `action` VARCHAR(32) NOT NULL,
        `account_id` INTEGER,
        `character_id` INTEGER,
        `payload` TEXT,
        `requested_by` VARCHAR(32) NOT NULL,
        `status` TINYINT NOT NULL DEFAULT 0,
        `result` TEXT,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `completed_at` DATETIME,
        PRIMARY KEY (`id`),
        KEY `admin_requests_status` (`status`, `id`)
    );

-- Server-side mutes (expires_at NULL = permanent; revoked_at set when lifted early)
CREATE TABLE
    IF NOT EXISTS `mutes` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `account_id` INTEGER NOT NULL,
        `reason` TEXT,
        `issued_by` VARCHAR(32) NOT NULL,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `expires_at` DATETIME,
        `revoked_at` DATETIME,
        `revoked_by` VARCHAR(32),
        PRIMARY KEY (`id`),
        KEY `mutes_account` (`account_id`)
    );

-- Formal warnings, shown in game until acknowledged
CREATE TABLE
    IF NOT EXISTS `warnings` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `account_id` INTEGER NOT NULL,
        `character_id` INTEGER,
        `category` VARCHAR(64) NOT NULL,
        `reason` TEXT NOT NULL,
        `issued_by` VARCHAR(32) NOT NULL,
        `report_id` INTEGER,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `delivered_at` DATETIME,
        `acknowledged_at` DATETIME,
        PRIMARY KEY (`id`),
        KEY `warnings_account` (`account_id`)
    );

-- Player reports from the website and from the game client
CREATE TABLE
    IF NOT EXISTS `reports` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `public_id` VARCHAR(16) NOT NULL,
        `token_hash` VARCHAR(160),
        `source` VARCHAR(8) NOT NULL,
        `reporter_account_id` INTEGER,
        `reporter_character_id` INTEGER,
        `reporter_name` VARCHAR(32),
        `contact_email` VARCHAR(254),
        `reported_character_id` INTEGER,
        `reported_name` VARCHAR(32) NOT NULL,
        `reported_account_name` VARCHAR(32),
        `category` VARCHAR(32) NOT NULL,
        `description` TEXT NOT NULL,
        `incident_at` DATETIME,
        `map_id` INTEGER,
        `x` INTEGER,
        `y` INTEGER,
        `status` VARCHAR(24) NOT NULL DEFAULT 'submitted',
        `assigned_to` VARCHAR(32),
        `public_response` TEXT,
        `reporter_ip` VARCHAR(45),
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `updated_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`id`),
        CONSTRAINT `reports_public_id_unique` UNIQUE (`public_id`),
        KEY `reports_status` (`status`),
        KEY `reports_reported` (`reported_character_id`)
    );

CREATE TABLE
    IF NOT EXISTS `report_attachments` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `report_id` INTEGER NOT NULL,
        `stored_name` VARCHAR(64) NOT NULL,
        `mime` VARCHAR(16) NOT NULL,
        `size` INTEGER NOT NULL,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`id`),
        KEY `report_attachments_report` (`report_id`)
    );

-- Disciplinary and account actions (history; enforcement lives in bans/mutes/warnings)
CREATE TABLE
    IF NOT EXISTS `moderation_actions` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `account_id` INTEGER NOT NULL,
        `character_id` INTEGER,
        `action` VARCHAR(24) NOT NULL,
        `reason` TEXT,
        `issued_by` VARCHAR(32) NOT NULL,
        `report_id` INTEGER,
        `request_id` INTEGER,
        `expires_at` DATETIME,
        `details` TEXT,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`id`),
        KEY `moderation_actions_account` (`account_id`)
    );

-- Item/gold adjustments made by staff, with before/after amounts
CREATE TABLE
    IF NOT EXISTS `item_adjustments` (
        `id` INTEGER NOT NULL AUTO_INCREMENT,
        `character_id` INTEGER NOT NULL,
        `item_id` INTEGER NOT NULL,
        `delta` INTEGER NOT NULL,
        `before_amount` INTEGER NOT NULL,
        `after_amount` INTEGER NOT NULL,
        `reason` TEXT NOT NULL,
        `issued_by` VARCHAR(32) NOT NULL,
        `request_id` INTEGER,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`id`),
        KEY `item_adjustments_character` (`character_id`)
    );

-- Server-wide switches (maintenance mode)
CREATE TABLE
    IF NOT EXISTS `server_flags` (
        `name` VARCHAR(32) NOT NULL,
        `value` VARCHAR(255) NOT NULL,
        `updated_by` VARCHAR(32),
        `updated_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (`name`)
    );
