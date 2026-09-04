package main

import (
	"strings"
	"testing"
	"unicode/utf8"
)

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
