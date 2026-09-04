package main

import (
	"strings"
	"testing"
)

func TestMatchesKeywordsScopesAndBlocks(t *testing.T) {
	msg := Message{Title: "Go release", Description: "New security fixes"}
	if got := matchesKeywords(msg, []string{"#tgo"}, "news"); len(got) != 1 || got[0] != "go" {
		t.Fatalf("title match failed: %#v", got)
	}
	if got := matchesKeywords(msg, []string{"#csecurity"}, "news"); len(got) != 1 || got[0] != "security" {
		t.Fatalf("description match failed: %#v", got)
	}
	if got := matchesKeywords(msg, []string{"go", "-security"}, "news"); len(got) != 0 {
		t.Fatalf("blocked message matched: %#v", got)
	}
}

func TestMatchesKeywordsRSSFilter(t *testing.T) {
	msg := Message{Title: "Technology news"}
	if got := matchesKeywords(msg, []string{"technology+Tech"}, "news"); len(got) != 0 {
		t.Fatalf("unexpected match for another RSS source: %#v", got)
	}
	if got := matchesKeywords(msg, []string{"technology+News"}, "news"); len(got) != 1 {
		t.Fatalf("expected case-insensitive RSS match: %#v", got)
	}
}

func TestCleanHTMLContentRemovesUnsafeLinks(t *testing.T) {
	clean := cleanHTMLContent(`<b>Title</b><a href="javascript:alert(1)">bad</a><a href="https://example.com">good</a>`)
	if strings.Contains(clean, "javascript:") {
		t.Fatalf("unsafe link survived sanitization: %s", clean)
	}
	if strings.Contains(clean, "bad</a>") {
		t.Fatalf("rejected link left a dangling closing tag: %s", clean)
	}
	if !strings.Contains(clean, `href="https://example.com"`) {
		t.Fatalf("safe link was removed: %s", clean)
	}
}
