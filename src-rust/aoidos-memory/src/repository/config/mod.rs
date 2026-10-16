//! 记忆算法配置使用版本行和当前指针，更新不改写已有周目的冻结策略。

use super::*;
use crate::model::{
    AppConfig, ConfigSaved, PolicyOverrides, PolicyParameters, ScriptConfig, valid_script,
};

const APPLICATION_SCOPE: &str = "application";
const APPLICATION_ID: &str = "default";
const SCRIPT_SCOPE: &str = "script";
const APP_CONFIG_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigPayload<T> {
    version: u32,
    algorithm_version: u32,
    value: T,
}

impl Repository {
    /// 读取当前应用默认；未保存时返回版本化开发初值，不写入数据库。
    /// # Errors
    /// 已保存配置的哈希、版本或参数无效时返回 `store.corrupt`。
    pub fn app_config(&self, conn: &Connection) -> Result<AppConfig> {
        read_app_config(conn)
    }

    /// CAS 保存应用默认，并检查现存剧本覆盖在新默认下仍可组成合法策略。
    /// # Errors
    /// 版本冲突、参数组合非法或现有覆盖与新默认冲突时拒绝保存。
    pub fn save_app_config(
        &self,
        conn: &mut Connection,
        expected_version: u64,
        parameters: PolicyParameters,
    ) -> Result<ConfigSaved> {
        parameters
            .clone()
            .into_policy(expected_version, expected_version)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let current = read_app_config(&tx)?;
        if current.config_version != expected_version {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let next_version = current
            .config_version
            .checked_add(1)
            .filter(|version| *version <= crate::model::MAX_INTEGER)
            .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
        validate_script_overrides(&tx, &parameters, next_version)?;
        ensure_initial_app_version(&tx, &current)?;
        insert_config(
            &tx,
            APPLICATION_SCOPE,
            APPLICATION_ID,
            next_version,
            APP_CONFIG_VERSION,
            &parameters,
        )?;
        tx.execute(
            "INSERT INTO memory_config_heads(scope_kind,scope_id,config_version) VALUES(?1,?2,?3) ON CONFLICT(scope_kind,scope_id) DO UPDATE SET config_version=excluded.config_version",
            params![APPLICATION_SCOPE, APPLICATION_ID, next_version],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(ConfigSaved {
            config_version: next_version,
        })
    }

    /// 原子读取剧本覆盖与应用默认，返回当前继承后的完整配置预览。
    /// # Errors
    /// 剧本 ID 非法、持久覆盖损坏或合并后的策略不合法时失败。
    pub fn script_config(&self, conn: &Connection, script_id: &str) -> Result<ScriptConfig> {
        if !valid_script(script_id) {
            return Err(reject());
        }
        let tx = conn.unchecked_transaction().map_err(sql)?;
        let app = read_app_config(&tx)?;
        let (config_version, overrides) = read_script_override(&tx, script_id)?;
        let effective = app.parameters.apply(&overrides);
        effective
            .clone()
            .into_policy(config_version, app.config_version)?;
        tx.commit().map_err(sql)?;
        Ok(ScriptConfig {
            script_id: script_id.into(),
            config_version,
            app_config_version: app.config_version,
            algorithm_version: APP_CONFIG_VERSION,
            overrides,
            inherited: app.parameters,
            effective,
        })
    }

    /// CAS 保存显式覆盖；应用默认变化会使旧草稿失效，空覆盖表示继承默认。
    /// # Errors
    /// 当前覆盖版或应用默认版与预期不符，或完整策略不合法时拒绝提交。
    pub fn save_script_config(
        &self,
        conn: &mut Connection,
        script_id: &str,
        expected_version: u64,
        expected_app_version: u64,
        overrides: PolicyOverrides,
    ) -> Result<ConfigSaved> {
        if !valid_script(script_id) {
            return Err(reject());
        }
        overrides.validate()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let app = read_app_config(&tx)?;
        let (current_version, _) = read_script_override(&tx, script_id)?;
        if app.config_version != expected_app_version || current_version != expected_version {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let effective = app.parameters.apply(&overrides);
        let next_version = current_version
            .checked_add(1)
            .filter(|version| *version <= crate::model::MAX_INTEGER)
            .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
        effective
            .clone()
            .into_policy(next_version, app.config_version)?;
        insert_config(
            &tx,
            SCRIPT_SCOPE,
            script_id,
            next_version,
            APP_CONFIG_VERSION,
            &overrides,
        )?;
        tx.execute(
            "INSERT INTO memory_config_heads(scope_kind,scope_id,config_version) VALUES(?1,?2,?3) ON CONFLICT(scope_kind,scope_id) DO UPDATE SET config_version=excluded.config_version",
            params![SCRIPT_SCOPE, script_id, next_version],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(ConfigSaved {
            config_version: next_version,
        })
    }
}

fn read_app_config(conn: &Connection) -> Result<AppConfig> {
    let Some(version) = current_version(conn, APPLICATION_SCOPE, APPLICATION_ID)? else {
        return Ok(AppConfig {
            config_version: 1,
            algorithm_version: APP_CONFIG_VERSION,
            parameters: PolicyParameters::development_defaults(),
            default_parameters: PolicyParameters::development_defaults(),
        });
    };
    let parameters: PolicyParameters = read_config_payload(
        conn,
        APPLICATION_SCOPE,
        APPLICATION_ID,
        version,
        APP_CONFIG_VERSION,
    )?;
    parameters.clone().into_policy(version, version)?;
    Ok(AppConfig {
        config_version: version,
        algorithm_version: APP_CONFIG_VERSION,
        parameters,
        default_parameters: PolicyParameters::development_defaults(),
    })
}

fn read_script_override(conn: &Connection, script_id: &str) -> Result<(u64, PolicyOverrides)> {
    let Some(version) = current_version(conn, SCRIPT_SCOPE, script_id)? else {
        return Ok((0, PolicyOverrides::default()));
    };
    let overrides: PolicyOverrides =
        read_config_payload(conn, SCRIPT_SCOPE, script_id, version, APP_CONFIG_VERSION)?;
    overrides.validate()?;
    Ok((version, overrides))
}

fn current_version(conn: &Connection, scope: &str, id: &str) -> Result<Option<u64>> {
    conn.query_row(
        "SELECT config_version FROM memory_config_heads WHERE scope_kind=?1 AND scope_id=?2",
        params![scope, id],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql)
}

fn read_config_payload<T: DeserializeOwned + Serialize>(
    conn: &Connection,
    scope: &str,
    id: &str,
    version: u64,
    expected_algorithm_version: u32,
) -> Result<T> {
    let (algorithm_version, payload_hash, payload): (u32, Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT algorithm_version,payload_hash,payload_json FROM memory_config_versions WHERE scope_kind=?1 AND scope_id=?2 AND config_version=?3",
            params![scope, id, version],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql)?;
    if payload.len() > 32 * 1024 || hash(&payload).as_slice() != payload_hash {
        return Err(Error::Corrupt);
    }

    // 先从已校验的规范 envelope 读取版本，避免把未来格式误报为损坏。
    let envelope: serde_json::Value =
        aoidos_json::decode_bytes(&payload, 32 * 1024).map_err(invalid_json)?;
    if canonical(&envelope)?.as_slice() != payload {
        return Err(Error::Corrupt);
    }
    let payload_version = envelope
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(Error::Corrupt)?;
    let payload_algorithm_version = envelope
        .get("algorithmVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(Error::Corrupt)?;
    if algorithm_version > expected_algorithm_version
        || payload_algorithm_version > expected_algorithm_version
        || payload_version > 1
    {
        return Err(Error::Rejected(crate::error::Reason::PolicyUnavailable));
    }
    if algorithm_version != expected_algorithm_version
        || payload_algorithm_version != expected_algorithm_version
        || payload_version != 1
    {
        return Err(Error::Corrupt);
    }
    let value: ConfigPayload<T> = aoidos_json::from_value(envelope).map_err(invalid_json)?;
    Ok(value.value)
}

fn insert_config<T: Serialize>(
    conn: &Connection,
    scope: &str,
    id: &str,
    version: u64,
    algorithm_version: u32,
    value: &T,
) -> Result<()> {
    let payload = canonical(&ConfigPayload {
        version: 1,
        algorithm_version,
        value,
    })?;
    if payload.len() > 32 * 1024 {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    conn.execute(
        "INSERT INTO memory_config_versions(scope_kind,scope_id,config_version,algorithm_version,payload_hash,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",
        params![scope, id, version, algorithm_version, hash(&payload).as_slice(), payload],
    )
    .map_err(sql)?;
    Ok(())
}

fn ensure_initial_app_version(conn: &Connection, app: &AppConfig) -> Result<()> {
    let exists = current_version(conn, APPLICATION_SCOPE, APPLICATION_ID)?.is_some();
    if !exists {
        insert_config(
            conn,
            APPLICATION_SCOPE,
            APPLICATION_ID,
            app.config_version,
            app.algorithm_version,
            &app.parameters,
        )?;
    }
    Ok(())
}

fn validate_script_overrides(
    conn: &Connection,
    new_defaults: &PolicyParameters,
    new_app_version: u64,
) -> Result<()> {
    let mut statement = conn
        .prepare(
            "SELECT h.scope_id,h.config_version FROM memory_config_heads h WHERE h.scope_kind='script' ORDER BY h.scope_id",
        )
        .map_err(sql)?;
    let mut rows = statement.query([]).map_err(sql)?;
    while let Some(row) = rows.next().map_err(sql)? {
        let (script_id, version): (String, u64) =
            (row.get(0).map_err(sql)?, row.get(1).map_err(sql)?);
        let overrides: PolicyOverrides =
            read_config_payload(conn, SCRIPT_SCOPE, &script_id, version, APP_CONFIG_VERSION)?;
        overrides.validate()?;
        new_defaults
            .apply(&overrides)
            .into_policy(version, new_app_version)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
