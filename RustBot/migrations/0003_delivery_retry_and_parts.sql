ALTER TABLE pending_deliveries
    ADD COLUMN next_attempt_at TEXT NOT NULL DEFAULT '';

CREATE INDEX idx_pending_deliveries_due
    ON pending_deliveries(subscription_id, next_attempt_at, created_at);

CREATE TABLE pending_delivery_parts (
    subscription_id INTEGER NOT NULL,
    entry_key TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    part_index INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('text_html', 'text_plain', 'photo')),
    content TEXT NOT NULL,
    media_url TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (subscription_id, entry_key, user_id, part_index),
    FOREIGN KEY (subscription_id, entry_key, user_id)
        REFERENCES pending_deliveries(subscription_id, entry_key, user_id) ON DELETE CASCADE
);

CREATE INDEX idx_pending_delivery_parts_delivery
    ON pending_delivery_parts(subscription_id, entry_key, user_id, part_index);
