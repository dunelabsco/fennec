---
name: linear
description: Query and mutate Linear issues, projects, and teams via the GraphQL API. Use for issue triage, status updates, ticket creation, and sprint queries. Requires the user's personal API key in the LINEAR_API_KEY env var.
always: false
---

# linear

Linear's API is GraphQL-only. Single endpoint, any operation, POST with a GraphQL query or mutation.

## First-time setup (user does this once)

1. In Linear, go to **Settings → Account → Security & Access**.
2. Click **New API key**. Name it (e.g. "Fennec"). Full access is simplest — restricted keys may hit authentication issues on some GraphQL operations.
3. Copy the key. It is shown once.
4. Save it: `export LINEAR_API_KEY=lin_api_...` in shell rc or the agent's config.

If `LINEAR_API_KEY` is missing, ask the user to complete setup.

## Calling the API

Endpoint:
```
POST https://api.linear.app/graphql
```

Headers:
```
Authorization: <LINEAR_API_KEY>
Content-Type: application/json
```

**Note:** personal API keys are passed bare, **without** the `Bearer` prefix. OAuth tokens do use `Bearer`; personal keys don't. Getting this wrong gives a 401.

Body:
```
{"query": "<GraphQL query or mutation>", "variables": {...}}
```

## Common operations

**Verify the key is live**
```graphql
{ viewer { id name email } }
```

**List my recent issues**
```graphql
{
  issues(first: 20,
         filter: {assignee: {isMe: {eq: true}}},
         orderBy: updatedAt) {
    nodes { id identifier title state { name } updatedAt }
  }
}
```

**Get one issue by identifier (e.g. `ENG-123`)**
```graphql
query($id: String!) {
  issue(id: $id) {
    id identifier title description
    state { name }
    assignee { name }
    labels { nodes { name } }
  }
}
```
Variables: `{"id": "ENG-123"}`

**Create an issue**
```graphql
mutation($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { id identifier url }
  }
}
```
Variables:
```
{"input": {"teamId": "<team_uuid>", "title": "...", "description": "..."}}
```

**Update issue status**
```graphql
mutation($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: {stateId: $stateId}) {
    success
    issue { state { name } }
  }
}
```

## Supporting queries you'll need first

- `teams { nodes { id name key } }` — team IDs / keys, required for issue creation.
- `workflowStates(filter: {team: {id: {eq: "<team_id>"}}}) { nodes { id name type } }` — state IDs for status updates.
- `users { nodes { id name email } }` — user IDs for reassignment.

## Failure modes

- `401 Unauthorized` → key is wrong, expired, or (common) you included `Bearer` in the header. Personal keys go bare.
- HTTP 200 with `errors: [...]` in the response body → GraphQL-level error. This is NOT a network failure. Always parse the `errors` array before trusting the response.
- Restricted key + privileged operation → regenerate with full access, or narrow the operation.
- Rate limiting → Linear's limits are generous but exist; pace requests.

## Rules

- Check the `errors` array every time. GraphQL errors come back 200 OK.
- Don't store the key in repo files. Use the env var.
- Prefer batched queries (one POST fetching multiple fields) over chained round-trips — GraphQL is for this.
