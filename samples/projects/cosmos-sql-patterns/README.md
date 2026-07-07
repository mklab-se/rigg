# cosmos-sql-patterns

Non-blob data sources with correct incremental indexing. These two policies
are the most common source of "why is my index stale/full of deleted rows".

## Cosmos DB (`hotel-reservations`)

- **Change detection**: `HighWaterMarkChangeDetectionPolicy` on `_ts` — and the
  query MUST filter `WHERE c._ts >= @HighWaterMark ORDER BY c._ts`, otherwise
  every run re-reads everything.
- **Deletion detection**: Cosmos deletes are invisible to the indexer; use a
  soft-delete marker column (`isDeleted: "true"`) and
  `SoftDeleteColumnDeletionDetectionPolicy`.
- **Auth**: identity-based connection (`ResourceId=...;IdentityAuthType=AccessToken`).
  Grant the search identity a Cosmos **SQL role** (not ARM RBAC):
  `az cosmosdb sql role assignment create ...` — `rigg auth doctor` reminds you.

## Azure SQL (`product-catalog`)

- **Change detection**: `SqlIntegratedChangeTrackingPolicy` — requires
  `ALTER DATABASE ... SET CHANGE_TRACKING = ON` and
  `ALTER TABLE [dbo].[Products] ENABLE CHANGE_TRACKING`. Handles deletes too,
  so no deletion policy is needed.
- **Auth**: create a contained AAD user for the search identity:
  `CREATE USER [your-search-service] FROM EXTERNAL PROVIDER;`
  `ALTER ROLE db_datareader ADD MEMBER [your-search-service];`

Push each piece as you verify it: `rigg push cosmos-sql-patterns`.
