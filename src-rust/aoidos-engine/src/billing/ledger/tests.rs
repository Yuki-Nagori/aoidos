use super::*;
use aoidos_billing::{
    money::{Currency, NanoMoney, UnitPrice},
    price::PriceVersion,
    schema::SCHEMA,
};
use rusqlite::Connection;

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    connection.execute_batch(SCHEMA).unwrap();
    connection
}

fn reservation(request_id: &str, currency: Currency, amount: &str) -> Reservation {
    Reservation {
        request_id: request_id.into(),
        run_id: "run-1".into(),
        turn_id: "turn-1".into(),
        profile_id: "profile-1".into(),
        attempt: 1,
        provider_id: "provider".into(),
        model_id: "model".into(),
        route_policy: "direct".into(),
        price_version_id: if currency == Currency::Cny {
            "price-1"
        } else {
            "price-2"
        }
        .into(),
        currency,
        dispatch_at: "2026-10-11T00:00:00Z".into(),
        amount: NanoMoney::parse(amount).unwrap(),
    }
}

fn prepare(connection: &mut Connection) {
    BillingLedger::set_settings(
        connection,
        "run-1",
        NanoMoney::parse("10").unwrap(),
        NanoMoney::parse("5").unwrap(),
        None,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    connection.execute(
        "INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,output_price,usage_mapping_version,source_kind,checked_at,effective_from,created_at) VALUES('price-1','provider','model','direct','CNY',1000,'1','1','1','v1','user','2026-10-10T00:00:00Z','2026-10-10T00:00:00Z','now')",
        [],
    ).unwrap();
    connection.execute(
        "INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,output_price,usage_mapping_version,source_kind,checked_at,effective_from,created_at) VALUES('price-2','provider','model','direct','USD',1000,'1','1','1','v1','user','2026-10-10T00:00:00Z','2026-10-10T00:00:00Z','now')",
        [],
    ).unwrap();
    BillingLedger::select_profile_price(
        connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
}

fn price() -> PriceVersion {
    PriceVersion {
        version_id: "price-register".into(),
        provider_id: "provider".into(),
        model_id: "model".into(),
        route_policy: "direct".into(),
        currency: Currency::Cny,
        unit_tokens: 1000,
        input_uncached: UnitPrice::parse("1.25").unwrap(),
        input_cached: UnitPrice::parse("0.5").unwrap(),
        output: UnitPrice::parse("4").unwrap(),
        usage_mapping_version: "v1".into(),
        source_kind: "user-entered".into(),
        source_url: None,
        checked_at: "2026-10-11T00:00:00Z".into(),
        effective_from: "2026-10-11T00:00:00Z".into(),
        valid_until: None,
    }
}

fn summary_settlement() -> Settlement {
    Settlement {
        input_total: 100,
        input_cached: 0,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 100,
        amount: NanoMoney::parse("0.1").unwrap(),
        result_digest: "sha256:settlement".into(),
    }
}

#[test]
fn error_display_settings_and_price_lookup_keep_domain_boundaries() {
    let errors = [
        (BillingError::Invalid, "invalid billing input"),
        (BillingError::LimitMissing, "run currency limit is missing"),
        (BillingError::Exceeded, "run currency budget exceeded"),
        (
            BillingError::PriceMissing,
            "no price is registered for the active profile route",
        ),
        (
            BillingError::PriceHistoryLimit,
            "price history reached its safety limit",
        ),
        (BillingError::NotFound, "billing request not found"),
        (
            BillingError::Conflict,
            "billing request conflicts with a previous operation",
        ),
        (
            BillingError::StaleRevision,
            "billing settings revision is stale",
        ),
        (BillingError::Storage, "billing storage operation failed"),
    ];
    for (error, display) in errors {
        assert_eq!(error.to_string(), display);
        let _: &dyn std::error::Error = &error;
    }

    let mut connection = database();
    assert_eq!(
        BillingLedger::get_settings(&connection, "run-1"),
        Err(BillingError::LimitMissing)
    );
    assert_eq!(
        BillingLedger::get_settings(&connection, ""),
        Err(BillingError::Invalid)
    );
    prepare(&mut connection);
    assert_eq!(
        BillingLedger::get_settings(&connection, "run-1")
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        BillingLedger::request_price(&connection, "absent"),
        Err(BillingError::NotFound)
    );
    assert_eq!(
        BillingLedger::price_by_id(&connection, "absent").unwrap(),
        None
    );
    assert_eq!(
        BillingLedger::price_by_id(&connection, "price-1")
            .unwrap()
            .unwrap()
            .version_id,
        "price-1"
    );
    assert_eq!(
        BillingLedger::active_price_for_currency(
            &connection,
            "unknown",
            "model",
            "direct",
            "2026-10-11T00:00:00Z",
            Currency::Cny
        )
        .unwrap(),
        None
    );
    let reservation = reservation("lookup", Currency::Cny, "1");
    BillingLedger::reserve(&mut connection, &reservation).unwrap();
    assert_eq!(
        BillingLedger::request_price(&connection, "lookup")
            .unwrap()
            .version_id,
        "price-1"
    );
    assert_eq!(
        BillingLedger::profile_price(
            &connection,
            "profile-1",
            "provider",
            "model",
            "direct",
            "2026-10-11T00:00:00Z"
        )
        .unwrap()
        .unwrap()
        .version_id,
        "price-1"
    );
}

#[test]
fn monthly_report_groups_settled_and_unknown_requests_by_original_currency() {
    let mut connection = database();
    prepare(&mut connection);
    let settled = reservation("month-settled", Currency::Cny, "1");
    BillingLedger::reserve(&mut connection, &settled).unwrap();
    let value = Settlement {
        input_total: 100,
        input_cached: 0,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 100,
        amount: NanoMoney::parse("0.1").unwrap(),
        result_digest: "sha256:month".into(),
    };
    BillingLedger::settle(
        &mut connection,
        &settled.request_id,
        Some(&value),
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::reserve(
        &mut connection,
        &reservation("month-unknown", Currency::Usd, "2"),
    )
    .unwrap();

    let items = BillingLedger::month_summary(&connection, "2026-10", "UTC").unwrap();
    assert_eq!(items.len(), 2);
    let cny = items
        .iter()
        .find(|item| item.currency == Currency::Cny)
        .unwrap();
    assert_eq!(cny.known_tokens, Some(100));
    assert_eq!(cny.settled_amount, Some(NanoMoney::parse("0.1").unwrap()));
    let usd = items
        .iter()
        .find(|item| item.currency == Currency::Usd)
        .unwrap();
    assert_eq!(usd.incomplete_requests, 1);
    assert_eq!(usd.reserved_amount, Some(NanoMoney::parse("2").unwrap()));
    assert_eq!(
        BillingLedger::month_summary(&connection, "2026-99", "UTC"),
        Err(BillingError::Invalid)
    );
    assert_eq!(
        BillingLedger::month_summary(&connection, "invalid", "UTC"),
        Err(BillingError::Invalid)
    );
}

#[test]
fn run_summary_omits_reservation_when_a_group_contains_only_settled_requests() {
    let mut connection = database();
    prepare(&mut connection);
    let request = reservation("settled-only", Currency::Cny, "1");
    BillingLedger::reserve(&mut connection, &request).unwrap();
    let settlement = Settlement {
        input_total: 100,
        input_cached: 0,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 100,
        amount: NanoMoney::parse("0.1").unwrap(),
        result_digest: "sha256:settled-only".into(),
    };
    BillingLedger::settle(
        &mut connection,
        &request.request_id,
        Some(&settlement),
        "2026-10-11T00:00:00Z",
    )
    .unwrap();

    let summary = BillingLedger::run_summary(&connection, "run-1").unwrap();
    assert_eq!(summary.len(), 1);
    assert_eq!(
        summary[0].settled_amount,
        Some(NanoMoney::parse("0.1").unwrap())
    );
    assert_eq!(summary[0].reserved_amount, None);
    assert_eq!(summary[0].known_tokens, Some(100));
}

#[test]
fn bounded_reports_reject_more_than_two_hundred_groups() {
    let mut connection = database();
    prepare(&mut connection);
    for index in 0..201 {
        connection.execute(
            "INSERT INTO billing_physical_requests(request_id,run_id,turn_id,profile_id,attempt,provider_id,model_id,route_policy,price_version_id,currency,dispatch_at,state,reserved_nanos,updated_at) VALUES(?1,'run-1','turn-1','profile-1',1,'provider',?2,'direct','price-1','CNY','2026-10-11T00:00:00Z','reserved',1,'now')",
            rusqlite::params![format!("group-{index}"), format!("model-{index}")],
        ).unwrap();
    }
    assert_eq!(
        BillingLedger::run_summary(&connection, "run-1"),
        Err(BillingError::Invalid)
    );
    assert_eq!(
        BillingLedger::month_summary(&connection, "2026-10", "UTC"),
        Err(BillingError::Invalid)
    );
}

#[test]
fn low_level_validation_adapters_reject_unrepresentable_inputs() {
    assert_eq!(super::validate_text("", 8), Err(BillingError::Invalid));
    assert_eq!(super::validate_text("x\n", 8), Err(BillingError::Invalid));
    assert_eq!(super::validate_text("12345", 4), Err(BillingError::Invalid));
    assert_eq!(super::to_i64(u64::MAX), Err(BillingError::Invalid));
    assert!(matches!(
        super::to_sql_error(aoidos_billing::money::MoneyError::Overflow),
        rusqlite::Error::IntegralValueOutOfRange(0, _)
    ));
    assert_eq!(
        super::storage(rusqlite::Error::InvalidQuery),
        BillingError::Storage
    );
    assert_eq!(
        super::parse_cursor(&"x".repeat(97)),
        Err(BillingError::Invalid)
    );
    assert_eq!(super::parse_cursor("r1;m2;a1"), Ok((1, 2, 1)));
    assert_eq!(super::parse_cursor("r0;m2;a1"), Err(BillingError::Invalid));
    assert_eq!(super::parse_cursor("r1;m2;a2"), Err(BillingError::Invalid));
    let mut invalid_reservation = reservation("bad-attempt", Currency::Cny, "1");
    invalid_reservation.attempt = 0;
    assert_eq!(
        super::validate_reservation(&invalid_reservation),
        Err(BillingError::Invalid)
    );
    let invalid_settlement = Settlement {
        input_total: 1,
        input_cached: 2,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 1,
        amount: NanoMoney::ZERO,
        result_digest: "digest".into(),
    };
    assert_eq!(
        super::validate_settlement(&invalid_settlement),
        Err(BillingError::Invalid)
    );

    let mut connection = database();
    assert_eq!(
        BillingLedger::set_settings(
            &mut connection,
            "run-1",
            NanoMoney::ZERO,
            NanoMoney::ZERO,
            Some(1),
            "now",
        ),
        Err(BillingError::StaleRevision)
    );
    connection
        .execute_batch("UPDATE billing_meta SET revision=9223372036854775807")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    assert_eq!(
        super::bump_revision(&transaction),
        Err(BillingError::Storage)
    );
}

#[test]
fn missing_prices_invalid_reservations_and_unknown_settlements_fail_closed() {
    let mut connection = database();
    prepare(&mut connection);
    assert_eq!(
        BillingLedger::select_profile_price(
            &mut connection,
            "profile",
            "provider",
            "unknown-model",
            "direct",
            Currency::Cny,
            "2026-10-11T00:00:00Z",
        ),
        Err(BillingError::NotFound)
    );
    let mut missing = reservation("missing-price", Currency::Cny, "1");
    missing.price_version_id = "absent".into();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &missing),
        Err(BillingError::NotFound)
    );
    let mut bad = reservation("bad-price-route", Currency::Cny, "1");
    bad.model_id = "other".into();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &bad),
        Err(BillingError::Invalid)
    );
    assert_eq!(
        BillingLedger::settle(&mut connection, "absent", None, "now"),
        Err(BillingError::NotFound)
    );

    let request = reservation("unknown-settlement", Currency::Cny, "1");
    BillingLedger::reserve(&mut connection, &request).unwrap();
    assert_eq!(
        BillingLedger::settle(&mut connection, &request.request_id, None, "now").unwrap(),
        SettlementResult::MarkedUnconfirmed
    );
    assert_eq!(
        BillingLedger::settle(&mut connection, &request.request_id, None, "later").unwrap(),
        SettlementResult::MarkedUnconfirmed
    );
    let value = Settlement {
        input_total: 100,
        input_cached: 0,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 100,
        amount: NanoMoney::parse("0.1").unwrap(),
        result_digest: "sha256:late".into(),
    };
    assert_eq!(
        BillingLedger::settle(
            &mut connection,
            &request.request_id,
            Some(&value),
            "recovered"
        )
        .unwrap(),
        SettlementResult::Applied
    );
    assert_eq!(
        BillingLedger::settle(&mut connection, &request.request_id, None, "later"),
        Err(BillingError::Conflict)
    );
}

#[test]
fn price_versions_are_immutable_and_idempotently_registered() {
    let mut connection = database();
    let value = price();
    BillingLedger::register_price(&mut connection, &value, "2026-10-11T00:00:01Z").unwrap();
    BillingLedger::register_price(&mut connection, &value, "2026-10-11T00:00:02Z").unwrap();
    let mut changed = value;
    changed.output = UnitPrice::parse("5").unwrap();
    assert_eq!(
        BillingLedger::register_price(&mut connection, &changed, "2026-10-11T00:00:03Z"),
        Err(BillingError::Conflict)
    );
}

#[test]
fn price_history_is_bounded_per_identity_and_keeps_idempotent_retries() {
    let mut connection = database();
    let mut existing = price();
    existing.version_id = "existing-at-limit".into();
    BillingLedger::register_price(&mut connection, &existing, "created-0").unwrap();
    connection
        .execute_batch(
            "WITH RECURSIVE versions(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM versions WHERE n<1023) \
             INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,output_price,usage_mapping_version,source_kind,checked_at,effective_from,created_at) \
             SELECT 'bulk-'||n,'provider','model','direct','CNY',1000,'1','1','1','v1','user-entered','2026-10-11T00:00:00Z','2026-10-11T00:00:00Z','created-'||n FROM versions;",
        )
        .unwrap();

    BillingLedger::register_price(&mut connection, &existing, "retry-at-limit").unwrap();
    let mut next = existing.clone();
    next.version_id = "version-1025".into();
    assert_eq!(
        BillingLedger::register_price(&mut connection, &next, "created-next"),
        Err(BillingError::PriceHistoryLimit)
    );

    connection
        .execute(
            "INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,output_price,usage_mapping_version,source_kind,checked_at,effective_from,created_at) VALUES('raw-over-limit','provider','model','direct','CNY',1000,'1','1','1','v1','user-entered','2026-10-11T00:00:00Z','2026-10-11T00:00:00Z','zzzz')",
            [],
        )
        .unwrap();
    assert_eq!(
        BillingLedger::active_price_for_currency(
            &connection,
            "provider",
            "model",
            "direct",
            "2026-10-12T00:00:00Z",
            Currency::Cny,
        ),
        Err(BillingError::PriceHistoryLimit)
    );
}

#[test]
fn limits_are_atomic_and_independent_by_currency() {
    let mut connection = database();
    prepare(&mut connection);
    BillingLedger::reserve(&mut connection, &reservation("cny-1", Currency::Cny, "7")).unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::reserve(&mut connection, &reservation("usd-1", Currency::Usd, "5")).unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &reservation("cny-2", Currency::Cny, "4")),
        Err(BillingError::Exceeded)
    );
}

#[test]
fn reservation_is_idempotent_but_conflicting_identity_is_rejected() {
    let mut connection = database();
    prepare(&mut connection);
    let request = reservation("request-1", Currency::Cny, "2");
    BillingLedger::reserve(&mut connection, &request).unwrap();
    BillingLedger::reserve(&mut connection, &request).unwrap();
    let mut collision = request;
    collision.amount = NanoMoney::parse("3").unwrap();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &collision),
        Err(BillingError::Conflict)
    );
}

#[test]
fn unknown_usage_keeps_hold_and_exact_settlement_is_idempotent() {
    let mut connection = database();
    prepare(&mut connection);
    connection.execute(
        "UPDATE billing_price_versions SET input_uncached_price='40',input_cached_price='40',output_price='40' WHERE version_id='price-2'",
        [],
    ).unwrap();
    BillingLedger::reserve(&mut connection, &reservation("unknown", Currency::Cny, "8")).unwrap();
    assert_eq!(
        BillingLedger::settle(&mut connection, "unknown", None, "now").unwrap(),
        SettlementResult::MarkedUnconfirmed
    );
    assert_eq!(
        BillingLedger::reserve(&mut connection, &reservation("blocked", Currency::Cny, "3")),
        Err(BillingError::Exceeded)
    );

    let known = reservation("known", Currency::Usd, "1");
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::reserve(&mut connection, &known).unwrap();
    let settlement = Settlement {
        input_total: 100,
        input_cached: 25,
        output_total: 50,
        reasoning_included: Some(10),
        total_tokens: 150,
        amount: NanoMoney::parse("6").unwrap(),
        result_digest: "sha256:response-1".into(),
    };
    let mut invalid_amount = settlement.clone();
    invalid_amount.amount = NanoMoney::parse("5.99").unwrap();
    assert_eq!(
        BillingLedger::settle(&mut connection, "known", Some(&invalid_amount), "now"),
        Err(BillingError::Invalid)
    );
    assert_eq!(
        BillingLedger::settle(&mut connection, "known", Some(&settlement), "now").unwrap(),
        SettlementResult::Applied
    );
    assert_eq!(
        BillingLedger::settle(&mut connection, "known", Some(&settlement), "later").unwrap(),
        SettlementResult::AlreadyApplied
    );
    let mut collision = settlement;
    collision.result_digest = "sha256:response-2".into();
    assert_eq!(
        BillingLedger::settle(&mut connection, "known", Some(&collision), "later"),
        Err(BillingError::Conflict)
    );
    // The full overage is retained and prevents further USD traffic.
    assert_eq!(
        BillingLedger::reserve(
            &mut connection,
            &reservation("usd-next", Currency::Usd, "0")
        ),
        Err(BillingError::Exceeded)
    );
}

#[test]
fn settings_revision_is_optimistic_and_stale_writes_do_not_apply() {
    let mut connection = database();
    prepare(&mut connection);
    let updated = BillingLedger::set_settings(
        &mut connection,
        "run-1",
        NanoMoney::parse("12").unwrap(),
        NanoMoney::parse("6").unwrap(),
        Some(1),
        "later",
    )
    .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(
        BillingLedger::set_settings(
            &mut connection,
            "run-1",
            NanoMoney::parse("99").unwrap(),
            NanoMoney::parse("99").unwrap(),
            Some(1),
            "stale",
        ),
        Err(BillingError::StaleRevision)
    );
}

#[test]
fn eighty_percent_warning_is_deduplicated_and_run_summary_keeps_unknowns_visible() {
    let mut connection = database();
    prepare(&mut connection);
    BillingLedger::reserve(
        &mut connection,
        &reservation("near-limit", Currency::Cny, "8"),
    )
    .unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::reserve(&mut connection, &reservation("unknown", Currency::Usd, "4")).unwrap();
    BillingLedger::settle(&mut connection, "unknown", None, "later").unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    let known = reservation("known-summary", Currency::Cny, "1");
    BillingLedger::reserve(&mut connection, &known).unwrap();
    let known_usage = Settlement {
        input_total: 100,
        input_cached: 0,
        output_total: 0,
        reasoning_included: None,
        total_tokens: 100,
        amount: NanoMoney::parse("0.1").unwrap(),
        result_digest: "sha256:summary".into(),
    };
    BillingLedger::settle(
        &mut connection,
        &known.request_id,
        Some(&known_usage),
        "later",
    )
    .unwrap();
    let warnings = BillingLedger::warnings(&connection, "run-1").unwrap();
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings
            .iter()
            .all(|warning| warning.threshold_percent == 80)
    );
    assert!(
        BillingLedger::set_settings(
            &mut connection,
            "run-1",
            NanoMoney::parse("9").unwrap(),
            NanoMoney::parse("5").unwrap(),
            Some(1),
            "later",
        )
        .is_ok()
    );
    assert_eq!(
        BillingLedger::warnings(&connection, "run-1").unwrap().len(),
        2
    );

    let summary = BillingLedger::run_summary(&connection, "run-1").unwrap();
    let cny = summary
        .iter()
        .find(|item| item.currency == Currency::Cny)
        .unwrap();
    let usd = summary
        .iter()
        .find(|item| item.currency == Currency::Usd)
        .unwrap();
    assert_eq!(cny.settled_amount, Some(NanoMoney::parse("0.1").unwrap()));
    assert_eq!(cny.reserved_amount, Some(NanoMoney::parse("8").unwrap()));
    assert_eq!(cny.incomplete_requests, 1);
    assert_eq!(usd.settled_amount, None);
    assert_eq!(usd.incomplete_requests, 1);
}

#[test]
fn selected_price_is_unavailable_outside_its_effective_interval() {
    let mut connection = database();
    let mut expired = price();
    expired.valid_until = Some("2026-10-12T00:00:00Z".into());
    BillingLedger::register_price(&mut connection, &expired, "created").unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(
        BillingLedger::profile_price(
            &connection,
            "profile-1",
            "provider",
            "model",
            "direct",
            "2026-10-12T00:00:00Z",
        )
        .unwrap(),
        None
    );
}

#[test]
fn price_reads_propagate_missing_table_storage_errors() {
    let mut connection = database();
    connection
        .execute_batch("DROP TABLE billing_price_versions;")
        .unwrap();
    assert_eq!(
        BillingLedger::suggested_limits(
            &connection,
            "provider",
            "model",
            "direct",
            "2026-10-11T00:00:00Z",
        ),
        Err(BillingError::Storage)
    );
    assert_eq!(
        BillingLedger::select_profile_price(
            &mut connection,
            "profile-1",
            "provider",
            "model",
            "direct",
            Currency::Cny,
            "2026-10-11T00:00:00Z",
        ),
        Err(BillingError::Storage)
    );
}

#[test]
fn audit_failures_roll_back_price_and_settings_writes() {
    let mut connection = database();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_billing_audit BEFORE INSERT ON billing_audit
             BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;",
        )
        .unwrap();
    assert_eq!(
        BillingLedger::register_price(&mut connection, &price(), "created"),
        Err(BillingError::Storage)
    );
    assert_eq!(
        BillingLedger::price_by_id(&connection, "price-register"),
        Ok(None)
    );
    assert_eq!(
        BillingLedger::set_settings(
            &mut connection,
            "run-1",
            NanoMoney::parse("10").unwrap(),
            NanoMoney::parse("5").unwrap(),
            None,
            "updated",
        ),
        Err(BillingError::Storage)
    );
    assert_eq!(
        BillingLedger::get_settings(&connection, "run-1"),
        Err(BillingError::LimitMissing)
    );
}

#[test]
fn warning_storage_failure_rolls_back_the_request_reservation() {
    let mut connection = database();
    prepare(&mut connection);
    connection
        .execute_batch(
            "CREATE TRIGGER fail_budget_warning BEFORE INSERT ON billing_budget_warnings
             BEGIN SELECT RAISE(ABORT, 'injected warning failure'); END;",
        )
        .unwrap();
    assert_eq!(
        BillingLedger::reserve(
            &mut connection,
            &reservation("warning-failure", Currency::Cny, "8"),
        ),
        Err(BillingError::Storage)
    );
    let request_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM billing_physical_requests WHERE request_id='warning-failure'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(request_count, 0);
}

#[test]
fn settlement_rejects_corrupt_states_and_ignored_sql_updates() {
    let mut corrupt = database();
    prepare(&mut corrupt);
    BillingLedger::reserve(
        &mut corrupt,
        &reservation("corrupt-state", Currency::Cny, "1"),
    )
    .unwrap();
    corrupt
        .execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    corrupt
        .execute(
            "UPDATE billing_physical_requests SET state='corrupt' WHERE request_id='corrupt-state'",
            [],
        )
        .unwrap();
    assert_eq!(
        BillingLedger::settle(
            &mut corrupt,
            "corrupt-state",
            Some(&summary_settlement()),
            "settled"
        ),
        Err(BillingError::Conflict)
    );

    let mut ignored = database();
    prepare(&mut ignored);
    BillingLedger::reserve(
        &mut ignored,
        &reservation("ignored-update", Currency::Cny, "1"),
    )
    .unwrap();
    ignored.execute_batch(
        "CREATE TRIGGER ignore_billing_settlement BEFORE UPDATE OF state ON billing_physical_requests
         WHEN NEW.state='settled' BEGIN SELECT RAISE(IGNORE); END;",
    ).unwrap();
    assert_eq!(
        BillingLedger::settle(
            &mut ignored,
            "ignored-update",
            Some(&summary_settlement()),
            "settled"
        ),
        Err(BillingError::Conflict)
    );
}

#[test]
fn monthly_boundaries_use_persisted_iana_zone_and_dst_rules() {
    assert_eq!(
        super::month_range("2026-03", "America/New_York").unwrap(),
        (
            "2026-03-01T05:00:00Z".to_owned(),
            "2026-04-01T04:00:00Z".to_owned()
        )
    );
    assert!(super::month_range("2026-13", "America/New_York").is_err());
    assert!(super::month_range("2026-03", "No/Such_Zone").is_err());

    let mut connection = database();
    assert_eq!(
        BillingLedger::report_time_zone(&mut connection, "America/New_York").unwrap(),
        "America/New_York"
    );
    assert_eq!(
        BillingLedger::report_time_zone(&mut connection, "UTC").unwrap(),
        "America/New_York"
    );
    let mut fallback = database();
    assert_eq!(
        BillingLedger::report_time_zone(&mut fallback, "No/Such_Zone").unwrap(),
        "UTC"
    );
}

#[test]
fn monthly_reports_compare_fractional_dispatch_times_by_instant() {
    let mut connection = database();
    prepare(&mut connection);
    for (request_id, dispatch_at) in [
        ("october-last-fraction", "2026-10-31T23:59:59.999Z"),
        ("november-boundary", "2026-11-01T00:00:00Z"),
        ("november-fraction", "2026-11-01T00:00:00.123Z"),
    ] {
        let mut request = reservation(request_id, Currency::Cny, "1");
        request.dispatch_at = dispatch_at.into();
        BillingLedger::reserve(&mut connection, &request).unwrap();
    }

    let october = BillingLedger::month_summary(&connection, "2026-10", "UTC").unwrap();
    let november = BillingLedger::month_summary(&connection, "2026-11", "UTC").unwrap();
    assert_eq!(october[0].request_count, 1);
    assert_eq!(november[0].request_count, 2);
}

#[test]
fn price_selection_and_reservation_compare_offset_timestamps_by_instant() {
    let mut connection = database();
    BillingLedger::set_settings(
        &mut connection,
        "run-1",
        NanoMoney::parse("10").unwrap(),
        NanoMoney::parse("5").unwrap(),
        None,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    let mut offset_price = price();
    offset_price.version_id = "offset-price".into();
    offset_price.effective_from = "2026-10-11T00:00:00-08:00".into();
    BillingLedger::register_price(&mut connection, &offset_price, "created").unwrap();
    assert_eq!(
        BillingLedger::select_profile_price(
            &mut connection,
            "profile-1",
            "provider",
            "model",
            "direct",
            Currency::Cny,
            "2026-10-11T04:00:00Z",
        ),
        Err(BillingError::NotFound)
    );
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T08:00:00Z",
    )
    .unwrap();
    let mut request = reservation("offset-request", Currency::Cny, "1");
    request.price_version_id = "offset-price".into();
    request.dispatch_at = "2026-10-11T04:00:00Z".into();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &request),
        Err(BillingError::Invalid)
    );
}

#[test]
fn active_price_lookup_preserves_submillisecond_effective_boundaries() {
    let mut connection = database();
    prepare(&mut connection);
    let mut future_price = price();
    future_price.version_id = "submillisecond-future".into();
    future_price.checked_at = "2026-10-11T00:00:00.0004Z".into();
    future_price.effective_from = future_price.checked_at.clone();
    BillingLedger::register_price(&mut connection, &future_price, "created-later").unwrap();

    let just_before = BillingLedger::active_price_for_currency(
        &connection,
        "provider",
        "model",
        "direct",
        "2026-10-11T00:00:00.0001Z",
        Currency::Cny,
    )
    .unwrap()
    .unwrap();
    let at_boundary = BillingLedger::active_price_for_currency(
        &connection,
        "provider",
        "model",
        "direct",
        "2026-10-11T00:00:00.0004Z",
        Currency::Cny,
    )
    .unwrap()
    .unwrap();

    assert_eq!(just_before.version_id, "price-1");
    assert_eq!(at_boundary.version_id, "submillisecond-future");
}

#[test]
fn request_pages_are_bounded_and_stale_cursors_are_rejected() {
    let mut connection = database();
    prepare(&mut connection);
    BillingLedger::reserve(&mut connection, &reservation("page-1", Currency::Cny, "1")).unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::reserve(&mut connection, &reservation("page-2", Currency::Usd, "1")).unwrap();
    let first = BillingLedger::list_requests(&connection, "run-1", None, Some(1)).unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some());
    let second =
        BillingLedger::list_requests(&connection, "run-1", first.next_cursor.as_deref(), Some(1))
            .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].request_id, "page-2");
    assert!(second.next_cursor.is_none());

    let stale = first.next_cursor.unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-1",
        "provider",
        "model",
        "direct",
        Currency::Cny,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-usd",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:01Z",
    )
    .unwrap();
    BillingLedger::reserve(&mut connection, &reservation("page-3", Currency::Cny, "1")).unwrap();
    assert_eq!(
        BillingLedger::list_requests(&connection, "run-1", Some(&stale), Some(1)),
        Err(BillingError::StaleRevision)
    );
    assert_eq!(
        BillingLedger::list_requests(&connection, "run-1", Some("bad"), Some(1)),
        Err(BillingError::Invalid)
    );
    assert_eq!(
        BillingLedger::list_requests(&connection, "run-1", None, Some(201)),
        Err(BillingError::Invalid)
    );
}

#[test]
fn suggestions_use_each_native_currency_price_independently() {
    let mut connection = database();
    prepare(&mut connection);
    let suggestions = BillingLedger::suggested_limits(
        &connection,
        "provider",
        "model",
        "direct",
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(suggestions.reference_input_tokens, 2_000_000);
    assert_eq!(suggestions.reference_output_tokens, 500_000);
    assert_eq!(suggestions.cny.unwrap(), NanoMoney::parse("2500").unwrap());
    assert_eq!(suggestions.usd.unwrap(), NanoMoney::parse("2500").unwrap());
    let missing_currency = BillingLedger::suggested_limits(
        &connection,
        "provider",
        "unknown-model",
        "direct",
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(missing_currency.cny, None);
    assert_eq!(missing_currency.usd, None);
}

#[test]
fn selected_billing_currency_is_scoped_to_profile_and_checked_at_reservation() {
    let mut connection = database();
    prepare(&mut connection);
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-usd",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    BillingLedger::select_profile_price(
        &mut connection,
        "profile-usd",
        "provider",
        "model",
        "direct",
        Currency::Usd,
        "2026-10-11T00:00:01Z",
    )
    .unwrap();
    assert_eq!(
        BillingLedger::profile_price(
            &connection,
            "profile-1",
            "provider",
            "model",
            "direct",
            "2026-10-11T00:00:00Z",
        )
        .unwrap()
        .unwrap()
        .currency,
        Currency::Cny
    );
    assert_eq!(
        BillingLedger::profile_price(
            &connection,
            "profile-usd",
            "provider",
            "model",
            "direct",
            "2026-10-11T00:00:00Z",
        )
        .unwrap()
        .unwrap()
        .currency,
        Currency::Usd
    );
    let mut wrong_currency = reservation("wrong-currency", Currency::Usd, "0");
    assert_eq!(
        BillingLedger::reserve(&mut connection, &wrong_currency),
        Err(BillingError::Invalid)
    );
    wrong_currency.profile_id = "profile-usd".into();
    assert!(BillingLedger::reserve(&mut connection, &wrong_currency).is_ok());
    let audit_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM billing_audit WHERE operation_kind='profile-price-select'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audit_count, 2);
}

#[test]
fn empty_request_pages_and_unselected_profiles_do_not_invent_records() {
    let connection = database();
    let page = BillingLedger::list_requests(&connection, "unstarted", None, None).unwrap();
    assert!(page.items.is_empty());
    assert!(page.next_cursor.is_none());
    assert_eq!(
        BillingLedger::profile_price(
            &connection,
            "unselected",
            "provider",
            "model",
            "direct",
            "2026-10-11T00:00:00Z"
        )
        .unwrap(),
        None
    );
}

#[test]
fn reservation_rejects_expired_price_even_when_profile_selection_is_present() {
    let mut connection = database();
    prepare(&mut connection);
    connection
        .execute(
            "UPDATE billing_price_versions SET valid_until='2026-10-10T12:00:00Z'",
            [],
        )
        .unwrap();
    assert_eq!(
        BillingLedger::reserve(&mut connection, &reservation("expired", Currency::Cny, "1")),
        Err(BillingError::Invalid)
    );
}
