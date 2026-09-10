PRAGMA foreign_keys = ON;

CREATE TABLE app_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO app_metadata (key, value) VALUES ('schema_family', 'rust-v1');

CREATE TABLE subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rss_url TEXT NOT NULL UNIQUE,
    rss_name TEXT NOT NULL UNIQUE,
    channel INTEGER NOT NULL DEFAULT 0 CHECK (channel IN (0, 1))
);

CREATE TABLE user_subscriptions (
    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    PRIMARY KEY (subscription_id, user_id)
);

CREATE INDEX idx_user_subscriptions_user ON user_subscriptions(user_id);

CREATE TABLE keywords (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    value TEXT NOT NULL,
    UNIQUE (user_id, value)
);

CREATE INDEX idx_keywords_user ON keywords(user_id);

CREATE TABLE feed_cursors (
    subscription_id INTEGER PRIMARY KEY REFERENCES subscriptions(id) ON DELETE CASCADE,
    last_published_at TEXT NOT NULL,
    latest_title TEXT NOT NULL DEFAULT ''
);

CREATE TABLE seen_entries (
    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    entry_key TEXT NOT NULL,
    published_at TEXT NOT NULL,
    seen_at TEXT NOT NULL,
    PRIMARY KEY (subscription_id, entry_key)
);

CREATE TABLE pending_deliveries (
    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    entry_key TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    link TEXT NOT NULL,
    published_at TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    PRIMARY KEY (subscription_id, entry_key, user_id),
    FOREIGN KEY (subscription_id, user_id)
        REFERENCES user_subscriptions(subscription_id, user_id) ON DELETE CASCADE
);

CREATE INDEX idx_pending_deliveries_subscription
    ON pending_deliveries(subscription_id, created_at);

