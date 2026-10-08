# Ranking notes — café naïve École

The ranked index scores a query against weights the model computed offline.
A query for `parseJSONResponse` splits into word pieces exactly the way the
document did, so `HTTPServer`, `http_server` and `server2` reach the same
vocabulary the encoder saw.

Unicode that must survive: Łódź, ß, Ångström, žluťoučký, Việt, 日本語, 한국어,
снег, Ελληνικά, עברית, العربية, ไทย, हिन्दी, 🎉, €, ℃, § 8.

| Step | What it does |
|------|--------------|
| walk | lists the files dcgrep would index |
| encode | log(1 + relu(logits)), max over tokens |
| write | one JSONL document per file, atomically |

    https://example.com/a?b=c user@example.com #[derive(Debug)] {"k":[1,2]}
