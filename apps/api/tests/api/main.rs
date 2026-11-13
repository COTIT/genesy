//! Every API integration test in one binary, one module per feature file. One link instead of
//! ~100 (each binary was ~140 MB), and `cargo test` spreads all tests over every core.
//! New feature file: add it under `tests/api/` and declare it below; `every_file_is_a_module` fails otherwise.

#[path = "../common/mod.rs"]
mod common;

mod analytics;
mod app_keys;
mod assistant;
mod assistant_batches;
mod assistant_capability_auth;
mod assistant_farm_facts;
mod assistant_gaps;
mod assistant_identity;
mod assistant_mail_recipients;
mod assistant_order_invoice_quotes;
mod assistant_preconditions;
mod assistant_price_import;
mod assistant_product_quotes;
mod assistant_retention;
mod assistant_review_rows;
mod assistant_review_safety;
mod assistant_storefront_design;
mod attention;
mod attention_priorities;
mod body_limits;
mod bootstrap;
mod checkout;
mod cli_migrate;
mod currencies;
mod currency_rates;
mod custom_statuses;
mod custom_types;
mod db;
mod deal_pipelines;
mod deals;
mod demo_seed;
mod discounts;
mod email_profiles;
mod email_templates;
mod farm_bug_reports;
mod farm_catalogue;
mod farm_demo;
mod farm_documents;
mod farm_energy;
mod farm_loans;
mod farm_logins;
mod farm_mapping;
mod farm_market;
mod farm_packing;
mod farm_passbook;
mod farm_payouts;
mod farm_pue;
mod farm_records;
mod farm_registration;
mod farm_sales;
mod farm_supply;
mod farm_supply_corrections;
mod farm_surveys;
mod farm_tenancy;
mod farm_tower;
mod farm_vouchers;
mod files;
mod flow_conversations;
mod flow_scenes;
mod health;
mod healthcheck;
mod hourly_rates;
mod invites;
mod invoice_actions;
mod invoice_credit;
mod invoice_money;
mod invoice_payments;
mod invoice_send;
mod invoices;
mod jobs;
mod load_deals;
mod load_jobs;
mod login;
mod lot_invoices;
mod lot_orders;
mod lot_passports;
mod lot_public;
mod lot_stock;
mod lots;
mod me;
mod member_removal;
mod numbering;
mod orders;
mod payments;
mod payments_stripe;
mod pdf_templates;
mod permissions;
mod phase1_pipes;
mod price_corrections;
mod pricing;
mod private_limits;
mod product_brands;
mod product_bulk_runs;
mod product_category_link;
mod product_fields;
mod product_images;
mod product_variants;
mod products;
mod quick_search;
mod rate_limit;
mod relations;
mod request_timeouts;
mod sales_groups;
mod scheduled_reports;
mod schema;
mod searches;
mod send_message;
mod settings;
mod settings_credentials;
mod shipment_methods;
mod signup;
mod special_offers;
mod stock;
mod stock_seed;
mod stock_transfer;
mod storage_locations;
mod storefront;
mod storefront_connections;
mod storefront_design;
mod storefront_design_edits;
mod storefront_preview_links;
mod supply_links;
mod teams;
mod till_cash;
mod till_catalog;
mod till_login;
mod till_pairing;
mod till_quote;
mod till_sales;
mod users;
mod vat_groups;
mod verify_email;
mod warehouse_hierarchy;
mod warehouse_locations;
mod warehouses;
mod webhook_endpoints;
mod webhook_events;
mod workspace_defaults;
mod workspaces;

/// A file in `tests/api/` without its `mod` line above would silently never compile or run.
#[test]
fn every_file_is_a_module() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/api");
    let main = include_str!("main.rs");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(module) = name.strip_suffix(".rs")
            && module != "main"
            && !main.lines().any(|l| l == format!("mod {module};"))
        {
            missing.push(module.to_owned());
        }
    }
    assert!(
        missing.is_empty(),
        "declare in tests/api/main.rs: {missing:?}"
    );
}

mod gateway_accounts;
mod invoice_gateway;

mod gateway_recovery;
