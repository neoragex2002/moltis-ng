export function normalizeSearchHits(rawHits) {
	var hits = Array.isArray(rawHits) ? rawHits : [];
	var result = [];
	for (var hit of hits) {
		var displayName = typeof hit.displayName === "string" ? hit.displayName.trim() : "";
		if (!displayName) {
			console.warn(
				'[moltis] event="session.contract_violation" reason_code="missing_display_name" decision="drop" policy="web_ui_session_owner_v1"',
				{ session_id: hit?.sessionId || null, surface: "search_hit" },
			);
			continue;
		}
		hit.displayName = displayName;
		result.push(hit);
	}
	return result;
}

