-- Admin control panel and player reports (Alduin). The website queues admin_requests; the game server
-- executes them with its own character/inventory/ban code and records the outcome.
CREATE TABLE
    IF NOT EXISTS `admin_requests` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `action` VARCHAR(32) NOT NULL,
        `account_id` INTEGER,
        `character_id` INTEGER,
        `payload` TEXT,
        `requested_by` VARCHAR(32) NOT NULL,
        `status` INTEGER NOT NULL DEFAULT 0,
        `result` TEXT,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `completed_at` DATETIME
    );

-- Server-side mutes (expires_at NULL = permanent; revoked_at set when lifted early)
CREATE TABLE
    IF NOT EXISTS `mutes` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `account_id` INTEGER NOT NULL,
        `reason` TEXT,
        `issued_by` VARCHAR(32) NOT NULL,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `expires_at` DATETIME,
        `revoked_at` DATETIME,
        `revoked_by` VARCHAR(32)
    );

-- Formal warnings, shown in game until acknowledged
CREATE TABLE
    IF NOT EXISTS `warnings` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `account_id` INTEGER NOT NULL,
        `character_id` INTEGER,
        `category` VARTEXT NOT NULL,
        `reason` TEXT NOT NULL,
        `issued_by` VARCHAR(32) NOT NULL,
        `report_id` INTEGER,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        `delivered_at` DATETIME,
        `acknowledged_at` DATETIME
    );

-- Player reports from the website and from the game client
CREATE TABLE
    IF NOT EXISTS `reports` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
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
        UNIQUE (`public_id`)
    );

CREATE TABLE
    IF NOT EXISTS `report_attachments` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `report_id` INTEGER NOT NULL,
        `stored_name` VARTEXT NOT NULL,
        `mime` VARCHAR(16) NOT NULL,
        `size` INTEGER NOT NULL,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

-- Disciplinary and account actions (history; enforcement lives in bans/mutes/warnings)
CREATE TABLE
    IF NOT EXISTS `moderation_actions` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `account_id` INTEGER NOT NULL,
        `character_id` INTEGER,
        `action` VARCHAR(24) NOT NULL,
        `reason` TEXT,
        `issued_by` VARCHAR(32) NOT NULL,
        `report_id` INTEGER,
        `request_id` INTEGER,
        `expires_at` DATETIME,
        `details` TEXT,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

-- Item/gold adjustments made by staff, with before/after amounts
CREATE TABLE
    IF NOT EXISTS `item_adjustments` (
        `id` INTEGER PRIMARY KEY AUTOINCREMENT,
        `character_id` INTEGER NOT NULL,
        `item_id` INTEGER NOT NULL,
        `delta` INTEGER NOT NULL,
        `before_amount` INTEGER NOT NULL,
        `after_amount` INTEGER NOT NULL,
        `reason` TEXT NOT NULL,
        `issued_by` VARCHAR(32) NOT NULL,
        `request_id` INTEGER,
        `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
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

CREATE INDEX IF NOT EXISTS admin_requests_status ON admin_requests (status, id);
CREATE INDEX IF NOT EXISTS mutes_account ON mutes (account_id);
CREATE INDEX IF NOT EXISTS warnings_account ON warnings (account_id);
CREATE INDEX IF NOT EXISTS reports_status ON reports (status);
CREATE INDEX IF NOT EXISTS moderation_actions_account ON moderation_actions (account_id);
CREATE INDEX IF NOT EXISTS item_adjustments_character ON item_adjustments (character_id);
