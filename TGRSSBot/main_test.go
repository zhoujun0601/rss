package main

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"unicode/utf8"
)

func TestLoadConfigRejectsNonPositiveCycle(t *testing.T) {
	workDir := t.TempDir()
	configPath := filepath.Join(workDir, ConfigFile)
	configBytes, err := json.Marshal(Config{BotToken: "test-token", Cycletime: 0})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(configPath, configBytes, 0600); err != nil {
		t.Fatal(err)
	}
	originalDir, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(workDir); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(originalDir) })

	if _, err := loadConfig(); err == nil {
		t.Fatal("expected non-positive Cycletime to be rejected")
	}
}

func TestSubscriptionURLUniqueConstraintIsAtomic(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "subscriptions.db")
	database, err := sql.Open("sqlite3", dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	database.SetMaxOpenConns(4)
	if _, err := database.Exec(`CREATE TABLE subscriptions (
		subscription_id INTEGER PRIMARY KEY AUTOINCREMENT,
		rss_url TEXT NOT NULL,
		rss_name TEXT NOT NULL UNIQUE,
		users TEXT NOT NULL DEFAULT ',',
		channel INTEGER DEFAULT 0
	)`); err != nil {
		t.Fatal(err)
	}
	if _, err := database.Exec(`CREATE TABLE feed_data (rss_name TEXT PRIMARY KEY, last_update_time TEXT, latest_title TEXT)`); err != nil {
		t.Fatal(err)
	}
	if err := ensureSubscriptionURLUnique(database); err != nil {
		t.Fatal(err)
	}

	var wg sync.WaitGroup
	results := make(chan error, 2)
	for i := 0; i < 2; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			_, insertErr := database.Exec("INSERT INTO subscriptions (rss_url, rss_name, users) VALUES (?, ?, ?)", "https://example.com/feed", fmt.Sprintf("feed-%d", i), "[1]")
			results <- insertErr
		}(i)
	}
	wg.Wait()
	close(results)

	successes := 0
	for insertErr := range results {
		if insertErr == nil {
			successes++
		}
	}
	if successes != 1 {
		t.Fatalf("expected exactly one concurrent insert to succeed, got %d", successes)
	}
	var count int
	if err := database.QueryRow("SELECT COUNT(*) FROM subscriptions WHERE rss_url = ?", "https://example.com/feed").Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 1 {
		t.Fatalf("expected one subscription row, got %d", count)
	}
}

func TestEnsureSubscriptionURLUniqueMergesLegacyRows(t *testing.T) {
	database, err := sql.Open("sqlite3", filepath.Join(t.TempDir(), "legacy.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	if _, err := database.Exec(`CREATE TABLE subscriptions (
		subscription_id INTEGER PRIMARY KEY AUTOINCREMENT,
		rss_url TEXT NOT NULL,
		rss_name TEXT NOT NULL UNIQUE,
		users TEXT NOT NULL DEFAULT ',',
		channel INTEGER DEFAULT 0
	)`); err != nil {
		t.Fatal(err)
	}
	if _, err := database.Exec(`CREATE TABLE feed_data (rss_name TEXT PRIMARY KEY, last_update_time TEXT, latest_title TEXT)`); err != nil {
		t.Fatal(err)
	}
	if _, err := database.Exec("INSERT INTO subscriptions (rss_url, rss_name, users, channel) VALUES (?, ?, ?, ?), (?, ?, ?, ?)",
		"https://example.com/feed", "old-a", "[1]", 0,
		"https://example.com/feed", "old-b", "[2]", 1); err != nil {
		t.Fatal(err)
	}
	if _, err := database.Exec("INSERT INTO feed_data (rss_name, last_update_time, latest_title) VALUES (?, ?, ?), (?, ?, ?)",
		"old-a", "2025-01-01 00:00:00", "old", "old-b", "2025-01-02 00:00:00", "new"); err != nil {
		t.Fatal(err)
	}
	if err := ensureSubscriptionURLUnique(database); err != nil {
		t.Fatal(err)
	}

	var count, channel int
	var users string
	if err := database.QueryRow("SELECT COUNT(*), MAX(channel), MAX(users) FROM subscriptions WHERE rss_url = ?", "https://example.com/feed").Scan(&count, &channel, &users); err != nil {
		t.Fatal(err)
	}
	if count != 1 || channel != 1 {
		t.Fatalf("legacy rows were not merged correctly: count=%d channel=%d", count, channel)
	}
	mergedUsers := parseUserIDs(users)
	if len(mergedUsers) != 2 || mergedUsers[0] != 1 || mergedUsers[1] != 2 {
		t.Fatalf("merged users mismatch: %#v", mergedUsers)
	}
	var latestTitle string
	if err := database.QueryRow("SELECT latest_title FROM feed_data WHERE rss_name = ?", "old-a").Scan(&latestTitle); err != nil {
		t.Fatal(err)
	}
	if latestTitle != "new" {
		t.Fatalf("expected latest feed cursor to be preserved, got %q", latestTitle)
	}
}

func TestSplitMessagePreservesUTF8(t *testing.T) {
	text := strings.Repeat("你好", 20)
	chunks := splitMessage(text, 17)
	if len(chunks) < 2 {
		t.Fatalf("expected multiple chunks, got %d", len(chunks))
	}
	if strings.Join(chunks, "") != text {
		t.Fatalf("chunks do not reconstruct the input")
	}
	for _, chunk := range chunks {
		if !utf8.ValidString(chunk) || len(chunk) > 17 {
			t.Fatalf("invalid or oversized chunk: %q", chunk)
		}
	}
}

func TestSplitMessageHandlesInvalidLimit(t *testing.T) {
	const text = "content"
	if got := splitMessage(text, 0); len(got) != 1 || got[0] != text {
		t.Fatalf("unexpected result for zero limit: %#v", got)
	}
}

func TestFormatHelpTextUsesCurrentProjectMetadata(t *testing.T) {
	text := formatHelpText(42)

	for _, expected := range []string{
		"TGBot_RSS RSS订阅机器人",
		"Go 编写的 RSS/Atom 订阅推送工具",
		"当前项目下载：42 次",
		"https://github.com/zhoujun0601/rss",
		"https://github.com/zhoujun0601/rss/issues",
	} {
		if !strings.Contains(text, expected) {
			t.Errorf("help text missing %q", expected)
		}
	}

	for _, legacy := range []string{"IonRh/TGBot_RSS", "IonMagic"} {
		if strings.Contains(text, legacy) {
			t.Errorf("help text still contains legacy project metadata %q", legacy)
		}
	}
}

func TestValidatePublicHTTPURLRejectsLocalTargets(t *testing.T) {
	for _, raw := range []string{"http://127.0.0.1/feed", "http://localhost/feed", "file:///tmp/feed", "http://user:pass@example.com/feed"} {
		if err := validatePublicHTTPURL(raw); err == nil {
			t.Errorf("expected URL to be rejected: %s", raw)
		}
	}
}

func TestValidatePublicHTTPURLAcceptsPublicIP(t *testing.T) {
	if err := validatePublicHTTPURL("https://8.8.8.8/feed"); err != nil {
		t.Fatalf("expected public URL to pass: %v", err)
	}
}
