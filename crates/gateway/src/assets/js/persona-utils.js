export function isPersonaListLoaded(personaIds) {
	return Array.isArray(personaIds) && personaIds.includes("default");
}

export function isPersonaMissing(configuredPersona, personaIds, personaIdsLoaded) {
	return Boolean(
		configuredPersona &&
			personaIdsLoaded &&
			Array.isArray(personaIds) &&
			!personaIds.includes(configuredPersona),
	);
}

