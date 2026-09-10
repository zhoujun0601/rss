CREATE TABLE user_entry_baselines (
    subscription_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    entry_key TEXT NOT NULL,
    PRIMARY KEY (subscription_id, user_id, entry_key),
    FOREIGN KEY (subscription_id, user_id)
        REFERENCES user_subscriptions(subscription_id, user_id) ON DELETE CASCADE
);

CREATE INDEX idx_user_entry_baselines_entry
    ON user_entry_baselines(subscription_id, entry_key);
