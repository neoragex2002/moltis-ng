export function isAgentListLoaded(agentIds) {
return Array.isArray(agentIds) && agentIds.includes("default");
}

export function isAgentMissing(configuredAgentId, agentIds, agentIdsLoaded) {
return Boolean(
configuredAgentId &&
agentIdsLoaded &&
Array.isArray(agentIds) &&
!agentIds.includes(configuredAgentId),
);
}
