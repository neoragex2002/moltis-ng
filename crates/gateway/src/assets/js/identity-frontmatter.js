// Identity frontmatter helpers (shared between UI and tests)

export function yamlScalar(value) {
	if (value == null) return "";
	var v = String(value);
	if (
		v.includes(":") ||
		v.includes("#") ||
		v.startsWith(" ") ||
		v.endsWith(" ") ||
		v.includes("\n")
	) {
		return `'${v.replaceAll("'", "''")}'`;
	}
	return v;
}

function normalizeNewlines(raw) {
	return String(raw || "").replaceAll("\r\n", "\n");
}

export function parseIdentityFrontmatter(raw) {
	var text = normalizeNewlines(raw);
	var m = text.match(/^---\n([\s\S]*?)\n---\n?/);
	if (!m) return {};
	var yaml = m[1] || "";
	var out = {};
	yaml.split("\n").forEach((line) => {
		var t = line.trim();
		if (!t || t.startsWith("#")) return;
		var idx = t.indexOf(":");
		if (idx === -1) return;
		var key = t.slice(0, idx).trim();
		var val = t.slice(idx + 1).trim();
		if (
			(val.startsWith('"') && val.endsWith('"')) ||
			(val.startsWith("'") && val.endsWith("'"))
		) {
			val = val.slice(1, -1);
		}
		val = val.replaceAll("''", "'");
		out[key] = val;
	});
	return out;
}

export function stripIdentityFrontmatter(raw) {
	var text = normalizeNewlines(raw);
	if (!text.startsWith("---\n")) return text;
	var end = text.indexOf("\n---\n", 4);
	if (end === -1) return text;
	return text.slice(end + 5);
}

export function upsertIdentityFrontmatter(raw, fields) {
	var body = stripIdentityFrontmatter(raw || "");
	if (!body.trim()) body = "\n# IDENTITY.md\n";

	var yamlLines = [];
	["name", "emoji", "creature", "vibe"].forEach((k) => {
		var v = (fields?.[k] || "").trim();
		if (v) yamlLines.push(`${k}: ${yamlScalar(v)}`);
	});
	var yaml = yamlLines.join("\n");
	return `---\n${yaml}\n---\n${body.startsWith("\n") ? "" : "\n"}${body}`;
}

