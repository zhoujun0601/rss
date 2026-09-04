package main

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"html"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"sync"
	"time"

	_ "github.com/mattn/go-sqlite3"
	"github.com/mmcdole/gofeed"
)

// 获取所有订阅
func getSubscriptions(db *sql.DB) ([]Subscription, error) {
	rows, err := db.Query("SELECT subscription_id, rss_url, rss_name, users, channel FROM subscriptions")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var subscriptions []Subscription
	for rows.Next() {
		var sub Subscription
		var usersStr string
		var channel int

		if err := rows.Scan(&sub.ID, &sub.URL, &sub.Name, &usersStr, &channel); err != nil {
			logMessage("error", fmt.Sprintf("读取订阅失败: %v", err))
			continue
		}

		// 解析用户ID列表
		sub.Users = parseUserIDs(usersStr)
		sub.Channel = channel
		subscriptions = append(subscriptions, sub)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}

	return subscriptions, nil
}

// 解析用户ID字符串
func parseUserIDs(usersStr string) []int64 {
	usersStr = strings.Trim(usersStr, "[] ")
	if usersStr == "" {
		return nil
	}

	var userIDs []int64
	for _, idStr := range strings.Split(usersStr, ",") {
		var id int64
		if n, _ := fmt.Sscanf(strings.TrimSpace(idStr), "%d", &id); n == 1 && id > 0 {
			userIDs = append(userIDs, id)
		}
	}
	return userIDs
}

// 获取用户关键词
func getUserKeywords(db *sql.DB) (map[int64][]string, error) {
	rows, err := db.Query("SELECT user_id, keywords FROM user_keywords")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	userKeywords := make(map[int64][]string)
	for rows.Next() {
		var userID int64
		var keywordsStr string

		if err := rows.Scan(&userID, &keywordsStr); err != nil {
			continue
		}

		// 解析关键词
		keywords := parseKeywords(keywordsStr)
		if len(keywords) > 0 {
			userKeywords[userID] = keywords
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}

	return userKeywords, nil
}

// 解析关键词字符串
func parseKeywords(keywordsStr string) []string {
	keywordsStr = strings.TrimSpace(keywordsStr)
	if keywordsStr == "" {
		return nil
	}

	// 如果是 JSON 数组格式
	if strings.HasPrefix(keywordsStr, "[") && strings.HasSuffix(keywordsStr, "]") {
		var keywords []string
		if err := json.Unmarshal([]byte(keywordsStr), &keywords); err == nil {
			return keywords
		}
	}

	// 如果不是 JSON 格式，按照逗号分割
	var keywords []string
	for _, kw := range strings.Split(keywordsStr, ",") {
		kw = strings.TrimSpace(kw)
		if kw != "" {
			keywords = append(keywords, kw)
		}
	}
	return keywords
}

// 获取RSS内容
func fetchRSS(db *sql.DB, sub Subscription, client *http.Client) ([]Message, error) {
	messages, latestTime, latestTitle, err := fetchRSSData(db, sub, client)
	if err != nil {
		return nil, err
	}
	if !latestTime.IsZero() {
		updateLastTime(db, sub.Name, latestTime, latestTitle)
	}
	return messages, nil
}

// fetchRSSData 获取新消息但不推进游标，由调用方在消息成功处理后提交游标。
func fetchRSSData(db *sql.DB, sub Subscription, client *http.Client) ([]Message, time.Time, string, error) {
	client = createRSSHTTPClient(client)
	parser := gofeed.NewParser()
	parser.Client = client

	// 获取RSS内容
	feed, err := parser.ParseURL(sub.URL)
	if err != nil {
		return nil, time.Time{}, "", err
	}

	if len(feed.Items) == 0 {
		return nil, time.Time{}, "", nil
	}

	// 获取上次更新时间
	lastUpdateTime, err := getLastUpdateTime(db, sub.Name)
	if err != nil {
		logMessage("error", fmt.Sprintf("获取更新时间失败: %v", err))
		return nil, time.Time{}, "", err
	}

	// 处理新消息
	var messages []Message
	var latestTime time.Time
	var latestTitle string

	for _, item := range feed.Items {
		pubTime := getItemTime(item)
		if pubTime.After(latestTime) {
			latestTime = pubTime
			latestTitle = item.Title
		}

		// 只添加新的内容
		if pubTime.After(lastUpdateTime) {
			messages = append(messages, Message{
				Title:       item.Title,
				Description: item.Description,
				Link:        item.Link,
				PubDate:     pubTime,
			})
		}
	}

	return messages, latestTime, latestTitle, nil
}

// 获取RSS项目的时间
func getItemTime(item *gofeed.Item) time.Time {
	if item.PublishedParsed != nil {
		return item.PublishedParsed.UTC()
	}
	if item.UpdatedParsed != nil {
		return item.UpdatedParsed.UTC()
	}
	return time.Now().UTC()
}

// 获取上次更新时间
func getLastUpdateTime(db *sql.DB, rssName string) (time.Time, error) {
	var timeStr string
	err := db.QueryRow("SELECT last_update_time FROM feed_data WHERE rss_name = ?", rssName).Scan(&timeStr)

	if err == sql.ErrNoRows {
		// 首次运行，插入记录
		_, err = db.Exec("INSERT INTO feed_data (rss_name, last_update_time, latest_title) VALUES (?, ?, ?)",
			rssName, time.Now().Format("2006-01-02 15:04:05"), "")
		return time.Time{}, err
	}

	if err != nil {
		return time.Time{}, err
	}

	return time.Parse("2006-01-02 15:04:05", timeStr)
}

// 更新最后更新时间
func updateLastTime(db *sql.DB, rssName string, updateTime time.Time, title string) {
	_, err := db.Exec("UPDATE feed_data SET last_update_time = ?, latest_title = ? WHERE rss_name = ?",
		updateTime.Format("2006-01-02 15:04:05"), title, rssName)
	if err != nil {
		logMessage("error", fmt.Sprintf("更新时间失败: %v", err))
	}
}

// 检查消息是否匹配关键词，返回匹配到的关键词列表
func matchesKeywords(msg Message, keywords []string, rssName string) []string {
	if len(keywords) == 0 {
		return nil
	}

	var matchedKeywords []string
	var blockedKeywords []string

	// 准备不同的匹配内容
	titleContent := strings.ToLower(msg.Title)
	descContent := strings.ToLower(msg.Description)
	allContent := strings.ToLower(msg.Title + " " + msg.Description)

	// 首先检查是否命中屏蔽词
	for _, keyword := range keywords {
		keyword = strings.TrimSpace(keyword)
		if keyword == "" {
			continue
		}

		// 检查是否是屏蔽关键词
		isBlockKeyword := strings.HasPrefix(keyword, "-")
		if isBlockKeyword {
			keyword = strings.TrimPrefix(keyword, "-")
			//fmt.Println("屏蔽关键词:", keyword)
		}

		// 检查匹配范围前缀 (#t=标题, #c=描述, #a=全部)
		var matchScope string = "default" // default表示只匹配标题（保持向后兼容）
		var processedKeyword string = keyword

		if strings.HasPrefix(keyword, "#t") {
			matchScope = "title"
			processedKeyword = strings.TrimPrefix(keyword, "#t")
		} else if strings.HasPrefix(keyword, "#c") {
			matchScope = "description"
			processedKeyword = strings.TrimPrefix(keyword, "#c")
		} else if strings.HasPrefix(keyword, "#a") {
			matchScope = "all"
			processedKeyword = strings.TrimPrefix(keyword, "#a")
		}

		// 移除前缀后可能存在的空格
		processedKeyword = strings.TrimSpace(processedKeyword)

		// 检查是否包含RSS名称限制 (格式: 关键词+rssname)
		var actualKeyword string
		var targetRSSName string
		hasRSSFilter := false

		if strings.Contains(processedKeyword, "+") {
			parts := strings.Split(processedKeyword, "+")
			if len(parts) == 2 {
				actualKeyword = strings.TrimSpace(parts[0])
				targetRSSName = strings.TrimSpace(parts[1])
				hasRSSFilter = true
			} else {
				actualKeyword = processedKeyword // 如果格式不正确，使用处理后的关键词
			}
		} else {
			actualKeyword = processedKeyword
		}

		// 如果指定了RSS名称过滤，检查当前RSS是否匹配
		if hasRSSFilter {
			if strings.ToLower(rssName) != strings.ToLower(targetRSSName) {
				continue // RSS名称不匹配，跳过此关键词
			}
		}

		// 将关键词转为小写
		lowerKeyword := strings.ToLower(actualKeyword)

		// 根据匹配范围选择要匹配的内容
		var targetContent string
		switch matchScope {
		case "title":
			targetContent = titleContent
		case "description":
			targetContent = descContent
		case "all":
			targetContent = allContent
		default: // 保持向后兼容，默认只匹配标题
			targetContent = titleContent
		}

		// 检查是否包含通配符
		if strings.Contains(lowerKeyword, "*") {
			//fmt.Println(lowerKeyword)
			// 将通配符转换为正则表达式
			pattern := strings.ReplaceAll(lowerKeyword, "*", ".*")
			pattern = "^.*" + pattern + ".*$"

			// 编译正则表达式
			re, err := regexp.Compile(pattern)
			if err == nil && re.MatchString(targetContent) {
				if isBlockKeyword {
					blockedKeywords = append(blockedKeywords, actualKeyword)
				} else {
					matchedKeywords = append(matchedKeywords, actualKeyword)
				}
				continue
			}
		}

		// 如果没有通配符或正则表达式失败，使用普通匹配
		if strings.Contains(targetContent, lowerKeyword) {
			if isBlockKeyword {
				blockedKeywords = append(blockedKeywords, actualKeyword)
			} else {
				matchedKeywords = append(matchedKeywords, actualKeyword)
			}
		}
	}

	// 如果命中任何屏蔽词，则返回空
	if len(blockedKeywords) > 0 {
		logMessage("debug", fmt.Sprintf("消息被屏蔽词[%s]过滤: %s",
			strings.Join(blockedKeywords, ", "), msg.Title))
		return nil
	}

	return matchedKeywords
}

type pendingDelivery struct {
	Sub    Subscription
	Msg    Message
	UserID int64
}

var pendingDeliveries = struct {
	sync.Mutex
	items map[string]pendingDelivery
}{items: make(map[string]pendingDelivery)}

func deliveryKey(subName string, msg Message, userID int64) string {
	return fmt.Sprintf("%s\x00%d\x00%s\x00%s\x00%s", subName, userID, msg.Link, msg.Title, msg.PubDate.UTC().Format(time.RFC3339Nano))
}

func queueDelivery(sub Subscription, msg Message, userID int64) {
	pendingDeliveries.Lock()
	pendingDeliveries.items[deliveryKey(sub.Name, msg, userID)] = pendingDelivery{Sub: sub, Msg: msg, UserID: userID}
	pendingDeliveries.Unlock()
}

func removeDelivery(subName string, msg Message, userID int64) {
	pendingDeliveries.Lock()
	delete(pendingDeliveries.items, deliveryKey(subName, msg, userID))
	pendingDeliveries.Unlock()
}

func pendingForSubscription(subName string) []pendingDelivery {
	pendingDeliveries.Lock()
	defer pendingDeliveries.Unlock()
	result := make([]pendingDelivery, 0)
	for _, item := range pendingDeliveries.items {
		if item.Sub.Name == subName {
			result = append(result, item)
		}
	}
	return result
}

func hasSubscriber(sub Subscription, userID int64) bool {
	for _, id := range sub.Users {
		if id == userID {
			return true
		}
	}
	return false
}

func deliverSubscriptionMessage(sub Subscription, msg Message, userID int64, matchedKeywords []string) error {
	keywordCodes := make([]string, len(matchedKeywords))
	for i, keyword := range matchedKeywords {
		keywordCodes[i] = fmt.Sprintf("<code>%s</code>", html.EscapeString(keyword))
	}
	formattedKeywords := strings.Join(keywordCodes, " ")
	formattedDate := msg.PubDate.In(time.FixedZone("CST", 8*60*60)).Format("2006-01-02 15:04:05")
	title := html.EscapeString(msg.Title)
	link := html.EscapeString(msg.Link)
	var htmlMessage, otherpush string
	var err error
	if sub.Channel == 1 {
		description := msg.Description
		cleanDescription := cleanHTMLContent(description)
		htmlMessage = fmt.Sprintf("👋 %s: %s\n🕒 %s\n%s\n", html.EscapeString(sub.Name), formattedKeywords, formattedDate, cleanDescription)
		otherpush = fmt.Sprintf("👋 %s\n🕒 %s\n%s", html.EscapeString(sub.Name), formattedDate, cleanDescription)
		if imageURL := extractImageURL(description); imageURL != "" {
			err = sendPhotoMessage(userID, imageURL, htmlMessage)
		} else {
			err = sendHTMLMessage(userID, htmlMessage)
		}
	} else {
		htmlMessage = fmt.Sprintf("📌 %s\n🔖 关键词: %s\n🕒 %s\n🔗 %s", title, formattedKeywords, formattedDate, link)
		otherpush = fmt.Sprintf("📌 %s\n🕒 %s\n🔗 %s", title, formattedDate, link)
		err = sendHTMLMessage(userID, htmlMessage)
	}
	if err != nil {
		return err
	}
	if globalConfig != nil && globalConfig.ADMINIDS != 0 && userID == globalConfig.ADMINIDS {
		go sendother(otherpush)
	}
	return nil
}

// 处理单个订阅
func processSubscription(db *sql.DB, sub Subscription, userKeywords map[int64][]string, client *http.Client) {
	if getCycleNum() == 0 {
		logMessage("info", fmt.Sprintf("处理订阅: %s (%s)", sub.Name, sub.URL))
	}
	pushCount := 0
	for _, pending := range pendingForSubscription(sub.Name) {
		if !hasSubscriber(sub, pending.UserID) {
			removeDelivery(sub.Name, pending.Msg, pending.UserID)
			continue
		}
		keywords := userKeywords[pending.UserID]
		matched := matchesKeywords(pending.Msg, keywords, sub.Name)
		if len(matched) == 0 {
			removeDelivery(sub.Name, pending.Msg, pending.UserID)
			continue
		}
		if err := deliverSubscriptionMessage(sub, pending.Msg, pending.UserID, matched); err != nil {
			logMessage("error", fmt.Sprintf("重试推送失败: %v", err), pending.UserID)
			continue
		}
		removeDelivery(sub.Name, pending.Msg, pending.UserID)
		pushCount++
		recordPush(sub.Name)
	}

	messages, latestTime, latestTitle, err := fetchRSSData(db, sub, client)
	if err != nil {
		logMessage("error", fmt.Sprintf("获取RSS失败 %s: %v", sub.Name, err))
		return
	}
	for _, msg := range messages {
		for _, userID := range sub.Users {
			matched := matchesKeywords(msg, userKeywords[userID], sub.Name)
			if len(matched) == 0 {
				continue
			}
			if err := deliverSubscriptionMessage(sub, msg, userID, matched); err != nil {
				queueDelivery(sub, msg, userID)
				logMessage("error", fmt.Sprintf("发送推送失败，将在下次检查重试: %v", err), userID)
				continue
			}
			pushCount++
			recordPush(sub.Name)
		}
	}
	if !latestTime.IsZero() {
		updateLastTime(db, sub.Name, latestTime, latestTitle)
	}
	if len(messages) == 0 {
		logMessage("debug", fmt.Sprintf("订阅 %s 无新内容", sub.Name))
	}
	logMessage("info", fmt.Sprintf("订阅 %s 完成，推送 %d 条消息", sub.Name, pushCount))
}

// 检查所有RSS订阅
func checkAllRSS(db *sql.DB) {
	if db == nil {
		logMessage("error", "检查RSS失败：数据库连接为空")
		return
	}
	var err error
	startTime := time.Now()
	resetPushStatsIfNeeded()
	logMessage("info", "开始检查RSS订阅...")

	// 获取数据
	subscriptions, err := getSubscriptions(db)
	if err != nil {
		logMessage("error", fmt.Sprintf("获取订阅失败: %v", err))
		return
	}

	if len(subscriptions) == 0 {
		logMessage("info", "没有找到RSS订阅")
		return
	}

	userKeywords, err := getUserKeywords(db)
	if err != nil {
		logMessage("error", fmt.Sprintf("获取用户关键词失败: %v", err))
		return
	}

	proxyURL := ""
	if globalConfig != nil {
		proxyURL = globalConfig.ProxyURL
	}
	client := createRSSHTTPClient(createHTTPClient(proxyURL))

	// 并发处理订阅
	var wg sync.WaitGroup
	for _, sub := range subscriptions {
		wg.Add(1)
		go func(sub Subscription) {
			defer wg.Done()
			processSubscription(db, sub, userKeywords, client)
		}(sub)
	}

	wg.Wait()
	logMessage("info", fmt.Sprintf("RSS检查完成，耗时: %v", time.Since(startTime)))
	setCycleNum(1)
	// 打印当前的推送统计
	//stats := GetPushStatsInfo()
	//if DailyPushStats.TotalPush > 0 {
	//	logMessage("info", stats)
	//}
}

// extractImageURL 从HTML内容中提取第一个图片URL
func extractImageURL(htmlContent string) string {
	// 1. 正则表达式匹配img标签的src属性
	imgRegex := regexp.MustCompile(`<img[^>]+src=["']([^"']+)["']`)
	matches := imgRegex.FindStringSubmatch(htmlContent)

	if len(matches) > 1 {
		return matches[1] // 返回第一个捕获组（图片URL）
	}

	// 2. 尝试在文本中直接寻找图片URL（.jpg, .png, .gif等格式）
	urlRegex := regexp.MustCompile(`https?://[^\s"']+\.(jpg|jpeg|png|gif|webp)`)
	urlMatches := urlRegex.FindString(htmlContent)

	if urlMatches != "" {
		return urlMatches
	}

	// 3. 检查Telegram CDN链接
	cdnRegex := regexp.MustCompile(`https?://cdn[0-9]*\.cdn-telegram\.org/[^\s"']+`)
	cdnMatches := cdnRegex.FindString(htmlContent)

	if cdnMatches != "" {
		return cdnMatches
	}

	// 没有找到图片，返回空字符串
	return ""
}

// cleanHTMLContent 清理HTML内容，移除Telegram不支持的标签
func cleanHTMLContent(htmlContent string) string {
	// 1. 移除img标签，但保留其它内容
	imgRegex := regexp.MustCompile(`<img[^>]*>`)
	content := imgRegex.ReplaceAllString(htmlContent, "")

	// 2. 替换<br>标签为换行符
	brRegex := regexp.MustCompile(`<br\s*\/?>`)
	content = brRegex.ReplaceAllString(content, "\n")

	// 3. 保留Telegram支持的标签，移除其他标签
	// Telegram支持的标签: <b>, <i>, <u>, <s>, <a>, <code>, <pre>
	// 我们采用分步骤处理的方式

	// 暂时标记支持的标签，以便后面恢复
	content = regexp.MustCompile(`<b>`).ReplaceAllString(content, "§§§B§§§")
	content = regexp.MustCompile(`</b>`).ReplaceAllString(content, "§§§/B§§§")
	content = regexp.MustCompile(`<i>`).ReplaceAllString(content, "§§§I§§§")
	content = regexp.MustCompile(`</i>`).ReplaceAllString(content, "§§§/I§§§")
	content = regexp.MustCompile(`<u>`).ReplaceAllString(content, "§§§U§§§")
	content = regexp.MustCompile(`</u>`).ReplaceAllString(content, "§§§/U§§§")
	content = regexp.MustCompile(`<s>`).ReplaceAllString(content, "§§§S§§§")
	content = regexp.MustCompile(`</s>`).ReplaceAllString(content, "§§§/S§§§")
	content = regexp.MustCompile(`<code>`).ReplaceAllString(content, "§§§CODE§§§")
	content = regexp.MustCompile(`</code>`).ReplaceAllString(content, "§§§/CODE§§§")
	content = regexp.MustCompile(`<pre>`).ReplaceAllString(content, "§§§PRE§§§")
	content = regexp.MustCompile(`</pre>`).ReplaceAllString(content, "§§§/PRE§§§")

	// 丢弃不安全链接时同时移除配对的结束标签，避免生成坏 HTML。
	anchorPairRegex := regexp.MustCompile(`(?is)<a\s+href=["']([^"']+)["'][^>]*>(.*?)</a>`)
	content = anchorPairRegex.ReplaceAllStringFunc(content, func(tag string) string {
		matches := anchorPairRegex.FindStringSubmatch(tag)
		if len(matches) != 3 || !isAllowedTelegramLink(matches[1]) {
			if len(matches) == 3 {
				return matches[2]
			}
			return ""
		}
		return tag
	})

	// 特殊处理a标签
	aTagRegex := regexp.MustCompile(`(?i)<a\s+href=["']([^"']+)["'][^>]*>`)
	content = aTagRegex.ReplaceAllStringFunc(content, func(tag string) string {
		matches := aTagRegex.FindStringSubmatch(tag)
		if len(matches) != 2 || !isAllowedTelegramLink(matches[1]) {
			return ""
		}
		link, _ := url.Parse(strings.TrimSpace(matches[1]))
		return "§§§A§§§" + html.EscapeString(link.String()) + "§§§"
	})
	content = regexp.MustCompile(`</a>`).ReplaceAllString(content, "§§§/A§§§")

	// 移除所有剩余的HTML标签
	allTagsRegex := regexp.MustCompile(`<[^>]*>`)
	content = allTagsRegex.ReplaceAllString(content, "")

	// 恢复支持的标签
	content = regexp.MustCompile(`§§§B§§§`).ReplaceAllString(content, "<b>")
	content = regexp.MustCompile(`§§§/B§§§`).ReplaceAllString(content, "</b>")
	content = regexp.MustCompile(`§§§I§§§`).ReplaceAllString(content, "<i>")
	content = regexp.MustCompile(`§§§/I§§§`).ReplaceAllString(content, "</i>")
	content = regexp.MustCompile(`§§§U§§§`).ReplaceAllString(content, "<u>")
	content = regexp.MustCompile(`§§§/U§§§`).ReplaceAllString(content, "</u>")
	content = regexp.MustCompile(`§§§S§§§`).ReplaceAllString(content, "<s>")
	content = regexp.MustCompile(`§§§/S§§§`).ReplaceAllString(content, "</s>")
	content = regexp.MustCompile(`§§§CODE§§§`).ReplaceAllString(content, "<code>")
	content = regexp.MustCompile(`§§§/CODE§§§`).ReplaceAllString(content, "</code>")
	content = regexp.MustCompile(`§§§PRE§§§`).ReplaceAllString(content, "<pre>")
	content = regexp.MustCompile(`§§§/PRE§§§`).ReplaceAllString(content, "</pre>")
	content = regexp.MustCompile(`§§§A§§§(.*?)§§§`).ReplaceAllString(content, `<a href="$1">`)
	content = regexp.MustCompile(`§§§/A§§§`).ReplaceAllString(content, "</a>")

	// 4. 移除连续的换行符
	multipleNewlinesRegex := regexp.MustCompile(`\n{3,}`)
	content = multipleNewlinesRegex.ReplaceAllString(content, "\n\n")

	return content
}

func isAllowedTelegramLink(rawLink string) bool {
	link, err := url.Parse(strings.TrimSpace(rawLink))
	return err == nil && (link.Scheme == "http" || link.Scheme == "https" || link.Scheme == "tg") && link.Host != ""
}
