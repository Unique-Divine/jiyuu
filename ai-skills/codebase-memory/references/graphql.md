# GraphQL Across Repos

Requires the fork build ([setup.md](setup.md)). The upstream release names
client routes by operation only and has no server side, so it can't link a
client query to a resolver.

## How the Link Works

Both sides are keyed by **field path**, which comes from GraphQL itself:

```text
server: schema files (.graphql, .graphqls)    client: operation text
  type Query { perp: Perp! }                    query OpenTrades {
  type Perp { trades(...): [PerpTrade!]! }        perp { trades(...) { id } } }
        |                                               |
Route __graphql__Query.perp          <--- GRAPHQL_CALLS (field_path "perp")
Route __graphql__Query.perp.trades   <--- GRAPHQL_CALLS (field_path "perp.trades")
        ^ HANDLES                          CROSS_GRAPHQL_CALLS after linking
perpResolver.Trades (gqlgen) or the schema Field
```

- **Server routes** come from schema files: each root field
  (`Query`, `Mutation`, `Subscription`) and one level below it. Route props
  hold the declared type and `deprecated` with its reason.
- **Handlers:** for gqlgen servers, the resolver method answering the field
  (type `Perp`, field `trades` → `perpResolver.Trades`), with ownership read
  from `gqlgen.yml`. Fields bound to struct members, and non-gqlgen servers,
  fall back to the schema `Field`.
- **Client edges** come from operation text: string literals, template
  literals, Python triple-quoted strings, codegen `graphql()` documents,
  module constants, and `{ query }` objects passed to any function.
- **Not linkable:** queries assembled at runtime from field lists, and SDK
  calls that hold no GraphQL text (for example `client.user(args, fields)`).
- **Full indexes only for route changes:** editing a schema, resolver, or
  `gqlgen.yml` triggers a full rebuild on reindex, which the incremental path
  handles automatically.

## Questions to Ask

Run linking from each client with the server as target first ([setup.md](setup.md)).

**Impact of changing a field**: which client code reaches `Query.perp.trades`?

```cypher
MATCH (c)-[r:GRAPHQL_CALLS]->(rt:Route {qualified_name: "__graphql__Query.perp.trades"})
RETURN c.name, c.file_path, r.operation LIMIT 100
```

Run it in each client project. In the server project, `CROSS_GRAPHQL_CALLS`
edges show the same callers from the server's side.

**Who uses a resolver**: from the handler to its Route, then to callers:

```cypher
MATCH (h)-[:HANDLES]->(rt:Route)<-[x:CROSS_GRAPHQL_CALLS]-(c)
WHERE h.name = "Trades" RETURN rt.name, c.name, c.file_path LIMIT 100
```

**Unused API surface**: server field Routes with no incoming cross-repo edge:

```cypher
MATCH (rt:Route) WHERE rt.qualified_name STARTS WITH "__graphql__"
AND NOT ()-[:CROSS_GRAPHQL_CALLS]->(rt)
RETURN rt.name LIMIT 500
```

A field with no static caller may still be used by runtime-built queries or
other clients you haven't indexed; treat the list as candidates.

**Deprecated fields still in use**: Routes whose props say `deprecated`, with
callers:

```cypher
MATCH (c)-[:CROSS_GRAPHQL_CALLS]->(rt:Route)
WHERE rt.properties CONTAINS '"deprecated":true'
RETURN rt.name, c.name, c.file_path LIMIT 200
```

Property and label names can differ between versions; check
`get_graph_schema` if a query returns nothing, and page through results.
