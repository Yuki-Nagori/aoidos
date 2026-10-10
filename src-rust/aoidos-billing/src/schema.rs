//! 计费表结构；由引擎的共享存储服务统一执行。

/// 按价格、单局上限、物理请求和审计职责分别建表。
pub const SCHEMA: &str = r#"
CREATE TABLE billing_price_versions (
  version_id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL CHECK(length(provider_id) BETWEEN 1 AND 128),
  model_id TEXT NOT NULL CHECK(length(model_id) BETWEEN 1 AND 128),
  route_policy TEXT NOT NULL CHECK(length(route_policy) BETWEEN 1 AND 128),
  currency TEXT NOT NULL CHECK(currency IN ('CNY', 'USD')),
  unit_tokens INTEGER NOT NULL CHECK(unit_tokens > 0),
  input_uncached_price TEXT NOT NULL,
  input_cached_price TEXT NOT NULL,
  output_price TEXT NOT NULL,
  usage_mapping_version TEXT NOT NULL CHECK(length(usage_mapping_version) BETWEEN 1 AND 128),
  source_kind TEXT NOT NULL CHECK(length(source_kind) BETWEEN 1 AND 128),
  source_url TEXT,
  checked_at TEXT NOT NULL,
  effective_from TEXT NOT NULL,
  valid_until TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX billing_price_lookup ON billing_price_versions(provider_id, model_id, route_policy, currency, created_at, version_id);
CREATE TABLE billing_active_profile_prices (
  profile_id TEXT PRIMARY KEY CHECK(length(profile_id) BETWEEN 1 AND 128),
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  route_policy TEXT NOT NULL,
  currency TEXT NOT NULL CHECK(currency IN ('CNY', 'USD')),
  price_version_id TEXT NOT NULL REFERENCES billing_price_versions(version_id),
  selected_at TEXT NOT NULL
);
CREATE TABLE billing_run_budgets (
  run_id TEXT PRIMARY KEY,
  cny_limit_nanos INTEGER NOT NULL CHECK(cny_limit_nanos >= 0),
  usd_limit_nanos INTEGER NOT NULL CHECK(usd_limit_nanos >= 0),
  revision INTEGER NOT NULL CHECK(revision > 0),
  updated_at TEXT NOT NULL
);
CREATE TABLE billing_physical_requests (
  request_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES billing_run_budgets(run_id),
  turn_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK(attempt > 0),
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  route_policy TEXT NOT NULL,
  price_version_id TEXT NOT NULL REFERENCES billing_price_versions(version_id),
  currency TEXT NOT NULL CHECK(currency IN ('CNY', 'USD')),
  dispatch_at TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('reserved', 'settled', 'unconfirmed')),
  reserved_nanos INTEGER NOT NULL CHECK(reserved_nanos >= 0),
  input_total INTEGER,
  input_cached INTEGER,
  output_total INTEGER,
  reasoning_included INTEGER,
  total_tokens INTEGER,
  actual_nanos INTEGER CHECK(actual_nanos IS NULL OR actual_nanos >= 0),
  result_digest TEXT,
  updated_at TEXT NOT NULL
);
CREATE INDEX billing_requests_run ON billing_physical_requests(run_id, request_id);
CREATE INDEX billing_requests_period ON billing_physical_requests(dispatch_at, provider_id, currency);
CREATE TABLE billing_budget_warnings (
  run_id TEXT NOT NULL REFERENCES billing_run_budgets(run_id),
  currency TEXT NOT NULL CHECK(currency IN ('CNY', 'USD')),
  threshold_percent INTEGER NOT NULL CHECK(threshold_percent = 80),
  reached_at TEXT NOT NULL,
  read_at TEXT,
  PRIMARY KEY(run_id, currency, threshold_percent)
);
CREATE TABLE billing_audit (
  operation_id TEXT PRIMARY KEY,
  operation_kind TEXT NOT NULL CHECK(length(operation_kind) BETWEEN 1 AND 64),
  target_id TEXT NOT NULL CHECK(length(target_id) BETWEEN 1 AND 128),
  details_json TEXT NOT NULL CHECK(length(details_json) <= 8192),
  created_at TEXT NOT NULL
);
CREATE TABLE billing_meta (
  id INTEGER PRIMARY KEY CHECK(id=1),
  revision INTEGER NOT NULL CHECK(revision > 0),
  time_zone TEXT
);
INSERT INTO billing_meta(id,revision) VALUES(1,1);
"#;

/// 启用计费命令前检查必要的数据表与主键结构。
pub fn verify(connection: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('billing_price_versions','billing_active_profile_prices','billing_run_budgets','billing_physical_requests','billing_budget_warnings','billing_audit','billing_meta')",
        [],
        |row| row.get(0),
    )?;
    if count != 7 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let valid: i64 = connection.query_row(
        "SELECT count(*)=1 FROM pragma_table_info('billing_physical_requests') WHERE name='request_id' AND type='TEXT' AND pk=1",
        [],
        |row| row.get(0),
    )?;
    if valid == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_propagates_database_errors_from_both_queries() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

        for denied_table in ["sqlite_master", "pragma_table_info"] {
            let connection = rusqlite::Connection::open_in_memory().unwrap();
            connection.execute_batch(SCHEMA).unwrap();
            connection.authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(context.action, AuthAction::Read { table_name, .. } if table_name == denied_table) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }));
            let error = verify(&connection).unwrap_err();
            assert!(
                matches!(error, rusqlite::Error::SqliteFailure(..)),
                "{denied_table}: {error}"
            );
        }
    }

    #[test]
    fn verification_accepts_schema_and_rejects_missing_or_weakened_tables() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        assert!(verify(&connection).is_ok());

        let missing = rusqlite::Connection::open_in_memory().unwrap();
        assert!(matches!(
            verify(&missing),
            Err(rusqlite::Error::InvalidQuery)
        ));

        connection
            .execute_batch("DROP TABLE billing_physical_requests; CREATE TABLE billing_physical_requests(request_id TEXT);")
            .unwrap();
        assert!(matches!(
            verify(&connection),
            Err(rusqlite::Error::InvalidQuery)
        ));
    }
}
