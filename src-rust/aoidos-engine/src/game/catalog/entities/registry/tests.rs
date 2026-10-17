use super::*;
use crate::{
    game::catalog::{Node, Scene, ScenePosition, SceneRuleView},
    ports::Terminal,
    record::{facts::SceneNode, format::Body, history::HistoryPort, test_support::Fixture},
};

const RULE_ID: &str = "generated-npc";

struct Domain {
    connection: std::sync::Mutex<Connection>,
    seed: EntityDirectory,
    catalog: SceneCatalog,
    fail_entry: bool,
}

impl Domain {
    fn new() -> Self {
        Self::with_connection(Connection::open_in_memory().unwrap(), 20)
    }

    fn with_limit(max_instances: usize) -> Self {
        Self::with_connection(Connection::open_in_memory().unwrap(), max_instances)
    }

    fn with_connection(connection: Connection, max_instances: usize) -> Self {
        Self::with_seed(connection, seed(), max_instances)
    }

    fn with_seed(connection: Connection, seed: EntityDirectory, max_instances: usize) -> Self {
        let catalog = catalog(max_instances);
        connection
            .execute_batch(aoidos_store::applied::SCHEMA)
            .unwrap();
        connection.execute_batch(MIGRATION).unwrap();
        Self::with_existing(connection, seed, catalog)
    }

    fn with_existing(connection: Connection, seed: EntityDirectory, catalog: SceneCatalog) -> Self {
        Self {
            connection: std::sync::Mutex::new(connection),
            seed,
            catalog,
            fail_entry: false,
        }
    }

    fn directory(&mut self, session: &Session) -> Result<EntityDirectory, Fault> {
        EntityRegistry.snapshot(&mut self.connection.lock().unwrap(), session, &self.seed)
    }
}

impl crate::record::world::WorldPort for Domain {
    fn validate(&self, session: &Session, mutations: &[WorldMutation]) -> Result<(), Fault> {
        EntityRegistry.validate_group(
            &mut self.connection.lock().unwrap(),
            session,
            &self.seed,
            &self.catalog,
            mutations,
        )
    }

    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        aoidos_store::applied::contains(&self.connection.lock().unwrap(), id, hash)
            .map_err(Fault::from)
    }

    fn apply(
        &mut self,
        session: &Session,
        record_seq: u64,
        mutation: &WorldMutation,
        hash: &str,
    ) -> Result<(), Fault> {
        if self.fail_entry {
            self.connection
                .lock()
                .unwrap()
                .execute_batch(
                    "CREATE TEMP TRIGGER reject_entity BEFORE INSERT ON entity_registry_entries BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )
                .map_err(sql)?;
        }
        let mut connection = self.connection.lock().unwrap();
        let result = EntityRegistry.apply(
            &mut connection,
            session,
            &self.seed,
            &self.catalog,
            EntityCreationCommit {
                record_seq,
                mutation,
                content_hash: hash,
            },
        );
        if self.fail_entry {
            self.fail_entry = false;
            connection
                .execute_batch("DROP TRIGGER reject_entity")
                .map_err(sql)?;
        }
        result
    }
}

impl HistoryPort for Domain {
    fn verify_target(&self, _: &Session, _: u64) -> Result<(), Fault> {
        Ok(())
    }

    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        crate::record::world::WorldPort::applied(self, id, hash)
    }

    fn rebuild(
        &mut self,
        _: &Session,
        _: &[u64],
        operation: &str,
        hash: &str,
    ) -> Result<(), Fault> {
        aoidos_store::applied::apply_once(
            &mut self.connection.lock().unwrap(),
            operation,
            hash,
            &mut |_| Ok(()),
        )
        .map(|_| ())
        .map_err(Fault::from)
    }
}

fn seed() -> EntityDirectory {
    seed_at(
        1,
        vec![CanonicalEntity {
            id: "player".into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: "玩家".into(),
        }],
    )
}

fn seed_at(revision: u64, entries: Vec<CanonicalEntity>) -> EntityDirectory {
    EntityDirectory::new(format::hash(b"script"), revision, true, entries).unwrap()
}

fn catalog(max_instances: usize) -> SceneCatalog {
    let revision = format::hash(b"script");
    SceneCatalog::new(
        "catalog-1".into(),
        revision,
        BTreeMap::from([(
            "entry".into(),
            Node {
                kind: "scene".into(),
                title: "入口".into(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "entry".into(),
            Scene {
                position: ScenePosition {
                    scene_id: "entry".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "entry".into(),
                        title: "入口".into(),
                    }],
                },
                rules: SceneRuleView {
                    scene_id: "entry".into(),
                    catalog_revision: "catalog-1".into(),
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec![],
                actor_ids: vec!["player".into()],
                forced_check: None,
                dice_disabled: true,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap()
    .with_entity_creation_rules(vec![
        EntityCreationRule::new(
            RULE_ID.into(),
            EntityKind::Character,
            "global".into(),
            max_instances,
        )
        .unwrap(),
    ])
    .unwrap()
}

fn plan(narrative_seq: u64, mutations: Vec<WorldMutation>) -> Body {
    Body::System {
        code: "settlementPlanned".into(),
        message: "受规则授权的身份创建意图".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({
            "version": 1,
            "roundId": uuid::Uuid::new_v4().to_string(),
            "operationId": uuid::Uuid::new_v4().to_string(),
            "narrativeSeq": narrative_seq,
            "worldRevision": "0",
            "mutations": mutations,
        }),
    }
}

fn prepared(domain: &Domain, session: &Session, name: &str) -> WorldMutation {
    let directory = domain
        .seed
        .with_dynamic(domain.seed.revision(), vec![])
        .unwrap();
    domain
        .catalog
        .prepare_entity_creation_for_session(
            &session.header().session_id,
            RULE_ID,
            name,
            &directory,
        )
        .unwrap()
}

fn narration(session: &mut Session) {
    session
        .append(Body::Narration {
            text: "已确认的前文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
}

#[test]
fn ids_are_rust_generated_persisted_and_recovered_without_changing_script_revision() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&fixture.session).unwrap();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    let intent = decode_intent(&mutation).unwrap();
    assert!(valid_dynamic_id(&intent.entity_id));
    assert!(!intent.entity_id.contains("渡鸦"));
    let before = fixture.session.header().script_revision.clone();
    fixture
        .session
        .commit_world(plan(1, vec![mutation.clone()]), &mut domain)
        .unwrap();

    let snapshot = domain.directory(&fixture.session).unwrap();
    assert_eq!(fixture.session.header().script_revision, before);
    assert_eq!(snapshot.revision(), 2);
    assert_eq!(
        snapshot
            .resolve_exact(EntityKind::Character, "global", "渡鸦")
            .unwrap()
            .unwrap()
            .id,
        intent.entity_id
    );
    assert!(
        domain
            .applied(&mutation.mutation_id, &mutation_hash(&mutation).unwrap())
            .unwrap()
    );

    fixture
        .session
        .commit_world(plan(1, vec![mutation]), &mut domain)
        .unwrap();
    assert_eq!(domain.directory(&fixture.session).unwrap().revision(), 2);
}

#[test]
fn validation_snapshot_reads_applied_registry_and_rejects_revision_drift() {
    let fixture = Fixture::new();
    let mut domain = Domain::new();
    let expected = domain.directory(&fixture.session).unwrap();
    let mut connection = domain.connection.lock().unwrap();

    let actual = EntityRegistry
        .validation_snapshot(&mut connection, &fixture.session, &domain.seed)
        .unwrap();
    assert_eq!(actual.revision(), expected.revision());
    assert_eq!(actual.entries().count(), expected.entries().count());

    connection
        .execute(
            "UPDATE entity_registry_runs SET entity_revision = entity_revision + 1 WHERE session_id = ?1",
            [&fixture.session.header().session_id],
        )
        .unwrap();
    assert_eq!(
        EntityRegistry
            .validation_snapshot(&mut connection, &fixture.session, &domain.seed)
            .unwrap_err()
            .code,
        "store.corrupt"
    );
}

#[test]
fn first_creation_does_not_require_a_previous_snapshot_read() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");

    fixture
        .session
        .commit_world(plan(1, vec![mutation]), &mut domain)
        .unwrap();

    assert_eq!(domain.directory(&fixture.session).unwrap().revision(), 2);
}

#[test]
fn whole_group_is_rejected_before_append_when_registry_capacity_or_revision_would_overflow() {
    let mut entities = vec![CanonicalEntity {
        id: "player".into(),
        kind: EntityKind::Character,
        scope: "global".into(),
        canonical_name: "玩家".into(),
    }];
    for index in 0..(MAX_ENTITIES - 3) {
        entities.push(CanonicalEntity {
            id: format!("seed-{index}"),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: format!("seed-{index}"),
        });
    }
    let mut fixture = Fixture::new();
    let mut domain = Domain::with_seed(
        Connection::open_in_memory().unwrap(),
        seed_at(1, entities),
        20,
    );
    narration(&mut fixture.session);
    let abandoned = prepared(&domain, &fixture.session, "弃置身份");
    fixture
        .session
        .commit_world(plan(1, vec![abandoned]), &mut domain)
        .unwrap();
    let path = fixture.session.path_at(0).unwrap();
    let prefix_hash = format::hash(fixture.session.read(path[0]).unwrap().raw.as_bytes());
    fixture
        .session
        .commit_history_fork(
            Body::System {
                code: "historyFork".into(),
                message: "保留废弃身份的审计记录".into(),
                related_seq: None,
                turn_id: None,
                data: serde_json::json!({
                    "version": 1,
                    "operationId": uuid::Uuid::new_v4().to_string(),
                    "mode": "rewind",
                    "parentControlSeq": 0,
                    "targetSeq": path[0],
                    "prefixHash": prefix_hash,
                }),
            },
            &mut domain,
        )
        .unwrap();
    let first = prepared(&domain, &fixture.session, "渡鸦");
    let second = prepared(&domain, &fixture.session, "旅人");
    let event_seq_before_rejection = fixture.session.last_event_seq();

    assert!(
        fixture
            .session
            .commit_world(plan(1, vec![first, second]), &mut domain)
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), event_seq_before_rejection);
    assert!(!fixture.session.needs_recovery());
    let snapshot = domain.directory(&fixture.session).unwrap();
    assert_eq!(snapshot.revision(), 2);
    assert_eq!(snapshot.entries().count(), MAX_ENTITIES - 2);

    let mut fixture = Fixture::new();
    let mut domain = Domain::with_seed(
        Connection::open_in_memory().unwrap(),
        seed_at(
            MAX_SAFE_INTEGER - 1,
            vec![CanonicalEntity {
                id: "player".into(),
                kind: EntityKind::Character,
                scope: "global".into(),
                canonical_name: "玩家".into(),
            }],
        ),
        20,
    );
    narration(&mut fixture.session);
    let first = prepared(&domain, &fixture.session, "渡鸦");
    let second = prepared(&domain, &fixture.session, "旅人");

    assert!(
        fixture
            .session
            .commit_world(plan(1, vec![first, second]), &mut domain)
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), 1);
    assert!(!fixture.session.needs_recovery());
    assert_eq!(
        domain.directory(&fixture.session).unwrap().revision(),
        MAX_SAFE_INTEGER - 1
    );
}

#[test]
fn reopening_record_and_database_preserves_the_same_dynamic_identity() {
    let root =
        std::env::temp_dir().join(format!("aoidos-entity-registry-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let record_path = root.join("session.jsonl");
    let events = std::sync::Arc::new(crate::record::test_support::Events::default());
    let mut domain =
        Domain::with_connection(Connection::open(root.join("storage.sqlite")).unwrap(), 20);
    let header = crate::record::test_support::header();
    let mut session = Session::create(record_path.clone(), header, events.clone()).unwrap();
    domain.directory(&session).unwrap();
    narration(&mut session);
    let mutation = prepared(&domain, &session, "渡鸦");
    let identity = decode_intent(&mutation).unwrap().entity_id;
    session
        .commit_world(plan(1, vec![mutation]), &mut domain)
        .unwrap();
    drop(session);
    let seed = domain.seed.clone();
    let catalog = catalog(20);
    drop(domain);

    let mut reopened = Session::open(record_path, events).unwrap();
    let mut domain = Domain::with_existing(
        Connection::open(root.join("storage.sqlite")).unwrap(),
        seed,
        catalog,
    );
    reopened.recover_world(&mut domain).unwrap();
    let snapshot = domain.directory(&reopened).unwrap();
    assert_eq!(snapshot.revision(), 2);
    assert_eq!(
        snapshot
            .resolve_exact(EntityKind::Character, "global", "渡鸦")
            .unwrap()
            .unwrap()
            .id,
        identity
    );
    drop(reopened);
    drop(domain);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_directory_pending_intent_and_unknown_rule_cannot_create_visible_identity() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&fixture.session).unwrap();
    narration(&mut fixture.session);
    let incomplete = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        1,
        false,
        vec![domain.seed.entries().next().unwrap().clone()],
    )
    .unwrap();
    assert!(
        domain
            .catalog
            .prepare_entity_creation_for_session(
                &fixture.session.header().session_id,
                RULE_ID,
                "渡鸦",
                &incomplete,
            )
            .is_err()
    );
    assert!(
        domain
            .catalog
            .prepare_entity_creation_for_session(
                &fixture.session.header().session_id,
                "unregistered",
                "渡鸦",
                &domain.seed,
            )
            .is_err()
    );

    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    domain.fail_entry = true;
    assert!(
        fixture
            .session
            .commit_world(plan(1, vec![mutation]), &mut domain)
            .is_err()
    );
    assert!(fixture.session.needs_recovery());
    assert_eq!(
        domain.directory(&fixture.session).unwrap_err().code,
        "engine.invalid-phase"
    );
    assert_eq!(
        domain
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM entity_registry_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        domain
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM store_applied", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    fixture.session.recover_world(&mut domain).unwrap();
    assert_eq!(domain.directory(&fixture.session).unwrap().revision(), 2);
    fixture.session.recover_world(&mut domain).unwrap();
    assert_eq!(domain.directory(&fixture.session).unwrap().revision(), 2);
}

#[test]
fn duplicate_names_and_rule_limits_are_rejected_before_the_intent_is_appended() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&fixture.session).unwrap();
    narration(&mut fixture.session);
    let first = prepared(&domain, &fixture.session, "渡鸦");
    let second = prepared(&domain, &fixture.session, "渡鸦");
    assert!(
        fixture
            .session
            .commit_world(plan(1, vec![first, second]), &mut domain)
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), 1);
    assert!(!fixture.session.needs_recovery());
    assert_eq!(domain.directory(&fixture.session).unwrap().revision(), 1);

    let mut limited_fixture = Fixture::new();
    let mut limited_domain = Domain::with_limit(1);
    limited_domain.directory(&limited_fixture.session).unwrap();
    narration(&mut limited_fixture.session);
    let first = prepared(&limited_domain, &limited_fixture.session, "渡鸦");
    limited_fixture
        .session
        .commit_world(plan(1, vec![first]), &mut limited_domain)
        .unwrap();
    let second = prepared(&limited_domain, &limited_fixture.session, "旅人");
    assert!(
        limited_fixture
            .session
            .commit_world(plan(1, vec![second]), &mut limited_domain)
            .is_err()
    );
    assert_eq!(limited_fixture.session.last_event_seq(), 2);
    assert!(!limited_fixture.session.needs_recovery());
}

#[test]
fn dynamic_identity_is_scoped_to_each_game_session() {
    let mut first = Fixture::new();
    let mut second = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&first.session).unwrap();
    domain.directory(&second.session).unwrap();
    narration(&mut first.session);
    narration(&mut second.session);
    let first_mutation = prepared(&domain, &first.session, "渡鸦");
    let second_mutation = prepared(&domain, &second.session, "渡鸦");
    let first_id = decode_intent(&first_mutation).unwrap().entity_id;
    let second_id = decode_intent(&second_mutation).unwrap().entity_id;
    assert_ne!(first_id, second_id);

    first
        .session
        .commit_world(plan(1, vec![first_mutation]), &mut domain)
        .unwrap();
    second
        .session
        .commit_world(plan(1, vec![second_mutation]), &mut domain)
        .unwrap();
    assert_eq!(domain.directory(&first.session).unwrap().revision(), 2);
    assert_eq!(domain.directory(&second.session).unwrap().revision(), 2);
}

#[test]
fn branch_rewind_hides_abandoned_entity_but_keeps_monotonic_revision_and_audit_row() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&fixture.session).unwrap();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    fixture
        .session
        .commit_world(plan(1, vec![mutation]), &mut domain)
        .unwrap();
    let old_snapshot = domain.directory(&fixture.session).unwrap();
    assert!(old_snapshot.is_session_snapshot_for(&fixture.session));
    let path = fixture.session.path_at(0).unwrap();
    let prefix_hash = format::hash(fixture.session.read(path[0]).unwrap().raw.as_bytes());
    let fork = Body::System {
        code: "historyFork".into(),
        message: "从叙事前文建立新分支".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({
            "version": 1,
            "operationId": uuid::Uuid::new_v4().to_string(),
            "mode": "rewind",
            "parentControlSeq": 0,
            "targetSeq": path[0],
            "prefixHash": prefix_hash,
        }),
    };
    fixture
        .session
        .commit_history_fork(fork, &mut domain)
        .unwrap();

    assert!(!old_snapshot.is_session_snapshot_for(&fixture.session));
    let snapshot = domain.directory(&fixture.session).unwrap();
    assert_eq!(snapshot.revision(), 2);
    assert!(snapshot.is_session_snapshot_for(&fixture.session));
    assert!(
        snapshot
            .resolve_exact(EntityKind::Character, "global", "渡鸦")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        domain
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM entity_registry_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[test]
fn schema_verification_rejects_missing_or_weakened_tables() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(aoidos_store::applied::SCHEMA)
        .unwrap();
    connection.execute_batch(MIGRATION).unwrap();
    verify_schema(&connection).unwrap();
    connection
        .execute_batch("DROP TABLE entity_registry_entries")
        .unwrap();
    assert!(verify_schema(&connection).is_err());
}

#[test]
fn direct_replay_requires_original_record_and_hash() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    let hash = mutation_hash(&mutation).unwrap();
    fixture
        .session
        .commit_world(plan(1, vec![mutation.clone()]), &mut domain)
        .unwrap();
    let mut connection = domain.connection.lock().unwrap();
    for (seq, hash, expected) in [
        (2, hash.clone(), true),
        (1, hash, false),
        (2, format::hash(b"wrong"), false),
    ] {
        assert_eq!(
            EntityRegistry
                .apply(
                    &mut connection,
                    &fixture.session,
                    &domain.seed,
                    &domain.catalog,
                    EntityCreationCommit {
                        record_seq: seq,
                        mutation: &mutation,
                        content_hash: &hash
                    }
                )
                .is_ok(),
            expected
        );
    }
    assert_eq!(
        read_rows(&connection, &fixture.session, &domain.seed)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn preparation_rejects_stale_duplicate_and_invalid_names() {
    let fixture = Fixture::new();
    let domain = Domain::new();
    for name in ["玩家", "", " padded ", "control\n"] {
        assert!(
            domain
                .catalog
                .prepare_entity_creation_for_session(
                    &fixture.session.header().session_id,
                    RULE_ID,
                    name,
                    &domain.seed
                )
                .is_err()
        );
    }
    let stale = EntityDirectory::new(
        format::hash(b"stale"),
        1,
        true,
        domain.seed.entries().cloned().collect(),
    )
    .unwrap();
    assert!(
        domain
            .catalog
            .prepare_entity_creation_for_session(
                &fixture.session.header().session_id,
                RULE_ID,
                "渡鸦",
                &stale
            )
            .is_err()
    );
}

#[test]
fn malformed_intents_and_cross_session_or_catalog_reuse_are_rejected() {
    let fixture = Fixture::new();
    let domain = Domain::new();
    let valid = prepared(&domain, &fixture.session, "渡鸦");
    for field in [
        "version",
        "sessionId",
        "scriptRevision",
        "catalogRevision",
        "ruleId",
        "entityId",
        "scope",
        "canonicalName",
    ] {
        let mut mutation = valid.clone();
        mutation.data.payload[field] = if field == "version" {
            serde_json::json!(2)
        } else {
            serde_json::json!("")
        };
        assert!(decode_intent(&mutation).is_err(), "{field}");
    }
    for edit in 0..3 {
        let mut mutation = valid.clone();
        match edit {
            0 => mutation.data.kind = "unregistered".into(),
            1 => mutation.data.version = 2,
            _ => mutation.mutation_id = "invalid".into(),
        }
        assert!(decode_intent(&mutation).is_err());
    }
    let mut connection = domain.connection.lock().unwrap();
    for (field, value) in [
        ("sessionId", uuid::Uuid::new_v4().to_string()),
        ("scriptRevision", format::hash(b"stale")),
        ("catalogRevision", "stale".into()),
        ("ruleId", "unregistered".into()),
        ("scope", "elsewhere".into()),
    ] {
        let mut mutation = valid.clone();
        mutation.data.payload[field] = serde_json::json!(value);
        assert!(
            EntityRegistry
                .validate_group(
                    &mut connection,
                    &fixture.session,
                    &domain.seed,
                    &domain.catalog,
                    &[mutation]
                )
                .is_err(),
            "{field}"
        );
    }
    assert!(
        EntityRegistry
            .validate_group(
                &mut connection,
                &fixture.session,
                &domain.seed,
                &domain.catalog,
                &[valid.clone(), valid]
            )
            .is_err()
    );
}

#[test]
fn registry_rejects_weakened_schema_and_unavailable_database() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(MIGRATION).unwrap();
    connection.execute_batch("DROP TABLE entity_registry_entries; CREATE TABLE entity_registry_entries(entity_id TEXT)").unwrap();
    assert!(verify_schema(&connection).is_err());
    let fixture = Fixture::new();
    let mut connection = Connection::open_in_memory().unwrap();
    assert!(
        EntityRegistry
            .snapshot(&mut connection, &fixture.session, &seed())
            .is_err()
    );
    let stale = EntityDirectory::new(
        format::hash(b"stale"),
        1,
        true,
        seed().entries().cloned().collect(),
    )
    .unwrap();
    assert!(
        EntityRegistry
            .snapshot(&mut connection, &fixture.session, &stale)
            .is_err()
    );
}

#[test]
fn snapshots_reject_corrupted_receipts_and_record_sources() {
    for statement in [
        "UPDATE entity_registry_runs SET script_id='other'",
        "UPDATE entity_registry_runs SET seed_revision=2,entity_revision=2",
        "UPDATE entity_registry_runs SET entity_revision=3",
        "UPDATE entity_registry_entries SET entity_id='dynamic-invalid'",
        "UPDATE entity_registry_entries SET scope=' padded '",
        "UPDATE entity_registry_entries SET canonical_name=' padded '",
        "UPDATE entity_registry_entries SET rule_id='bad id'",
        "UPDATE entity_registry_entries SET mutation_hash='sha256:0000000000000000000000000000000000000000000000000000000000000000'",
        "UPDATE entity_registry_entries SET record_seq=1",
        "UPDATE entity_registry_entries SET entity_revision=3",
        "DELETE FROM store_applied",
        "UPDATE entity_registry_entries SET canonical_name='changed'",
        "UPDATE entity_registry_entries SET record_seq=999",
    ] {
        let mut fixture = Fixture::new();
        let mut domain = Domain::new();
        narration(&mut fixture.session);
        let mutation = prepared(&domain, &fixture.session, "渡鸦");
        fixture
            .session
            .commit_world(plan(1, vec![mutation]), &mut domain)
            .unwrap();
        {
            let connection = domain.connection.lock().unwrap();
            connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            connection.execute_batch(statement).unwrap();
        }
        assert!(domain.directory(&fixture.session).is_err(), "{statement}");
    }
}

#[test]
fn all_entity_kinds_roundtrip_and_unknown_kind_is_corrupt() {
    for kind in [
        EntityKind::Character,
        EntityKind::Place,
        EntityKind::Organization,
        EntityKind::Item,
        EntityKind::Term,
    ] {
        assert_eq!(parse_kind(kind_name(kind)).unwrap(), kind);
    }
    assert_eq!(parse_kind("unknown").unwrap_err().code, "store.corrupt");
}

#[test]
fn replay_rejects_identity_mismatch_and_abandoned_provenance() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    fixture
        .session
        .commit_world(plan(1, vec![mutation.clone()]), &mut domain)
        .unwrap();
    let rows = read_rows(
        &domain.connection.lock().unwrap(),
        &fixture.session,
        &domain.seed,
    )
    .unwrap();
    let mut edited = mutation.clone();
    edited.data.payload["canonicalName"] = serde_json::json!("changed");
    assert!(
        validate_group_rows(
            &rows,
            &fixture.session,
            &domain.seed,
            &domain.catalog,
            &[edited]
        )
        .is_err()
    );
    let mut off_branch = rows;
    off_branch.path.clear();
    assert!(
        validate_group_rows(
            &off_branch,
            &fixture.session,
            &domain.seed,
            &domain.catalog,
            &[mutation]
        )
        .is_err()
    );
    off_branch.revision = 0;
    assert!(
        validate_group_rows(
            &off_branch,
            &fixture.session,
            &domain.seed,
            &domain.catalog,
            &[]
        )
        .is_err()
    );
}

#[test]
fn record_provenance_requires_a_planned_matching_mutation() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    let hash = mutation_hash(&mutation).unwrap();
    fixture
        .session
        .commit_world(plan(1, vec![mutation.clone()]), &mut domain)
        .unwrap();
    let mut stored = read_rows(
        &domain.connection.lock().unwrap(),
        &fixture.session,
        &domain.seed,
    )
    .unwrap()
    .all
    .pop()
    .unwrap();
    let unrelated = fixture
        .session
        .append(Body::System {
            code: "orphan".into(),
            message: "Forward compatible record".into(),
            related_seq: None,
            turn_id: None,
            data: serde_json::json!({"version":1,"recoveryId":format::hash(b"recovery")}),
        })
        .unwrap();
    for seq in [1, unrelated] {
        stored.record_seq = seq;
        assert!(!record_contains_persisted(&fixture.session, &stored).unwrap());
        assert!(!record_contains_mutation(&fixture.session, seq, &mutation, &hash).unwrap());
    }
    stored.record_seq = 2;
    stored.mutation_id = uuid::Uuid::new_v4().to_string();
    assert!(!record_contains_persisted(&fixture.session, &stored).unwrap());
    let mut other = mutation.clone();
    other.mutation_id = uuid::Uuid::new_v4().to_string();
    assert!(!record_contains_mutation(&fixture.session, 2, &other, &hash).unwrap());
    let mut lines = std::fs::read_to_string(&fixture.path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut future: serde_json::Value = serde_json::from_str(&lines[unrelated as usize]).unwrap();
    future["kind"] = serde_json::json!("futureKind");
    lines[unrelated as usize] = serde_json::to_string(&future).unwrap();
    std::fs::write(&fixture.path, format!("{}\n", lines.join("\n"))).unwrap();
    let reopened = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
    stored.record_seq = unrelated;
    assert!(!record_contains_persisted(&reopened, &stored).unwrap());
    assert!(!record_contains_mutation(&reopened, unrelated, &mutation, &hash).unwrap());
}

#[test]
fn applied_receipt_without_entity_and_failed_revision_cas_are_rejected() {
    for fault in 0..2 {
        let mut fixture = Fixture::new();
        let mut domain = Domain::new();
        narration(&mut fixture.session);
        let mutation = prepared(&domain, &fixture.session, "渡鸦");
        let hash = mutation_hash(&mutation).unwrap();
        domain.fail_entry = true;
        assert!(
            fixture
                .session
                .commit_world(plan(1, vec![mutation.clone()]), &mut domain)
                .is_err()
        );
        let mut connection = domain.connection.lock().unwrap();
        if fault == 0 {
            connection
                .execute(
                    "INSERT INTO store_applied(id, content_hash) VALUES (?1,?2)",
                    params![mutation.mutation_id, hash],
                )
                .unwrap();
        } else {
            connection.execute_batch("CREATE TRIGGER reject_revision BEFORE UPDATE ON entity_registry_runs BEGIN SELECT RAISE(IGNORE); END").unwrap();
        }
        assert!(
            EntityRegistry
                .apply(
                    &mut connection,
                    &fixture.session,
                    &domain.seed,
                    &domain.catalog,
                    EntityCreationCommit {
                        record_seq: 2,
                        mutation: &mutation,
                        content_hash: &hash
                    }
                )
                .is_err()
        );
    }
    let fixture = Fixture::new();
    let domain = Domain::new();
    let connection = domain.connection.lock().unwrap();
    assert!(read_rows(&connection, &fixture.session, &domain.seed).is_err());
}

#[test]
fn oversized_corrupt_table_is_rejected_before_materializing_unbounded_audit_rows() {
    let mut fixture = Fixture::new();
    let mut domain = Domain::new();
    narration(&mut fixture.session);
    let mutation = prepared(&domain, &fixture.session, "渡鸦");
    fixture
        .session
        .commit_world(plan(1, vec![mutation]), &mut domain)
        .unwrap();
    {
        let connection = domain.connection.lock().unwrap();
        connection.execute_batch("CREATE TEMP TABLE original_entity AS SELECT * FROM entity_registry_entries; DROP TABLE entity_registry_entries; CREATE TABLE entity_registry_entries AS SELECT * FROM original_entity").unwrap();
        connection.execute(&format!("WITH RECURSIVE revisions(n) AS (VALUES(3) UNION ALL SELECT n+1 FROM revisions WHERE n < {}) INSERT INTO entity_registry_entries SELECT session_id,entity_id,kind,scope,canonical_name,rule_id,catalog_revision,mutation_id,mutation_hash,record_seq,n FROM original_entity,revisions", MAX_DYNAMIC_ENTITIES + 2), []).unwrap();
    }
    assert_eq!(
        domain.directory(&fixture.session).unwrap_err().code,
        "store.corrupt"
    );
}

#[test]
fn fresh_row_validation_rejects_seed_revision_mismatch_and_apply_rule_mismatch() {
    let fixture = Fixture::new();
    let mut domain = Domain::new();
    domain.directory(&fixture.session).unwrap();
    let mut connection = domain.connection.lock().unwrap();
    let other_seed = seed_at(2, domain.seed.entries().cloned().collect());
    assert!(read_rows(&connection, &fixture.session, &other_seed).is_err());
    let mut mutation = prepared(&domain, &fixture.session, "渡鸦");
    mutation.data.payload["catalogRevision"] = serde_json::json!("unregistered");
    assert!(
        EntityRegistry
            .apply(
                &mut connection,
                &fixture.session,
                &domain.seed,
                &domain.catalog,
                EntityCreationCommit {
                    record_seq: 1,
                    content_hash: &mutation_hash(&mutation).unwrap(),
                    mutation: &mutation
                }
            )
            .is_err()
    );
}

#[test]
fn registry_leaves_other_mutation_kinds_to_their_registered_interpreter() {
    let fixture = Fixture::new();
    let domain = Domain::new();
    let mutation = WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version: 1,
            kind: "sessionEnded".into(),
            payload: serde_json::json!({}),
        },
    };
    EntityRegistry
        .validate_group(
            &mut domain.connection.lock().unwrap(),
            &fixture.session,
            &domain.seed,
            &domain.catalog,
            &[mutation],
        )
        .unwrap();
}

#[test]
fn corrupt_negative_database_integers_and_untyped_intents_fail_closed() {
    for statement in [
        "UPDATE entity_registry_runs SET seed_revision=-1",
        "UPDATE entity_registry_runs SET entity_revision=-1",
        "UPDATE entity_registry_entries SET record_seq=-1",
        "UPDATE entity_registry_entries SET entity_revision=-1",
    ] {
        let mut fixture = Fixture::new();
        let mut domain = Domain::new();
        narration(&mut fixture.session);
        let mutation = prepared(&domain, &fixture.session, "渡鸦");
        fixture
            .session
            .commit_world(plan(1, vec![mutation]), &mut domain)
            .unwrap();
        let connection = domain.connection.lock().unwrap();
        connection
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        connection.execute_batch(statement).unwrap();
        assert!(
            read_rows(&connection, &fixture.session, &domain.seed).is_err(),
            "{statement}"
        );
    }
    let fixture = Fixture::new();
    let domain = Domain::new();
    let mut mutation = prepared(&domain, &fixture.session, "渡鸦");
    mutation.data.payload = serde_json::json!({});
    assert!(decode_intent(&mutation).is_err());
}
