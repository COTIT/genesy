CREATE TABLE product_bulk_runs (
 id uuid PRIMARY KEY DEFAULT uuidv7(), workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 created_by uuid NOT NULL REFERENCES users(id), name text NOT NULL DEFAULT 'Product changes', instruction text NOT NULL DEFAULT '',
 mode text NOT NULL DEFAULT 'create' CHECK(mode IN ('create','update','delete','upsert')),
 match_key text NOT NULL DEFAULT 'product_nr_formatted' CHECK(match_key IN ('product_nr_formatted','barcode','name','id','ean13','reference')),
 revision bigint NOT NULL DEFAULT 1, state text NOT NULL DEFAULT 'staged', mapping jsonb NOT NULL DEFAULT '{}',
 selection jsonb, selection_cursor uuid, selection_done boolean NOT NULL DEFAULT true,
 row_count bigint NOT NULL DEFAULT 0 CHECK(row_count BETWEEN 0 AND 50000), cell_count bigint NOT NULL DEFAULT 0 CHECK(cell_count BETWEEN 0 AND 2000000),
 prepare_cursor bigint NOT NULL DEFAULT 0, issues jsonb NOT NULL DEFAULT '[]', approved_until timestamptz, cancelled_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(), finished_at timestamptz,
 retain_until timestamptz NOT NULL DEFAULT now()+interval '30 days', UNIQUE(workspace_id,id)
);
CREATE INDEX product_bulk_runs_list ON product_bulk_runs(workspace_id,created_at DESC,id);
CREATE TABLE product_bulk_sources (
 id uuid PRIMARY KEY, workspace_id uuid NOT NULL, run_id uuid NOT NULL, object_key text NOT NULL UNIQUE,
 name text NOT NULL, media_type text NOT NULL, size_bytes bigint NOT NULL CHECK(size_bytes BETWEEN 1 AND 10485760), sha256 text NOT NULL,
 generation bigint NOT NULL DEFAULT 1, state text NOT NULL DEFAULT 'stored', cursor jsonb NOT NULL DEFAULT '{}', columns jsonb NOT NULL DEFAULT '{}', suggested_mapping jsonb NOT NULL DEFAULT '{}', issues jsonb NOT NULL DEFAULT '[]',
 created_at timestamptz NOT NULL DEFAULT now(), UNIQUE(workspace_id,id), UNIQUE(workspace_id,run_id,id),
 FOREIGN KEY(workspace_id,run_id) REFERENCES product_bulk_runs(workspace_id,id) ON DELETE CASCADE
);
CREATE TABLE product_bulk_rows (
 id uuid PRIMARY KEY DEFAULT uuidv7(), workspace_id uuid NOT NULL, run_id uuid NOT NULL, ordinal bigint NOT NULL,
 source_id uuid, sheet text, page integer, line integer, raw jsonb, provenance jsonb NOT NULL DEFAULT '{}', operation text NOT NULL CHECK(operation IN ('create','update','delete','upsert')),
 target jsonb NOT NULL DEFAULT '{}', values jsonb NOT NULL DEFAULT '{}', included boolean NOT NULL DEFAULT true,
 cell_count integer NOT NULL DEFAULT 0, selection_generated boolean NOT NULL DEFAULT false, selection_base jsonb, manual_override boolean NOT NULL DEFAULT false, status text NOT NULL DEFAULT 'staged', issues jsonb NOT NULL DEFAULT '[]', resolved_id uuid, material jsonb,
 validated_revision bigint, receipt jsonb, UNIQUE(run_id,ordinal), UNIQUE(run_id,id),
 FOREIGN KEY(workspace_id,run_id) REFERENCES product_bulk_runs(workspace_id,id) ON DELETE CASCADE,
 FOREIGN KEY(workspace_id,run_id,source_id) REFERENCES product_bulk_sources(workspace_id,run_id,id)
);
CREATE INDEX product_bulk_rows_page ON product_bulk_rows(run_id,ordinal);
CREATE INDEX product_bulk_rows_status ON product_bulk_rows(run_id,status,ordinal);
CREATE TABLE product_bulk_reviews (
 id uuid PRIMARY KEY DEFAULT uuidv7(), workspace_id uuid NOT NULL, run_id uuid NOT NULL, revision bigint NOT NULL,
 scope text NOT NULL CHECK(scope IN ('write','delete')), fingerprint text NOT NULL, row_count bigint NOT NULL,
 state text NOT NULL DEFAULT 'pending', approved_by uuid REFERENCES users(id), approved_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL DEFAULT now()+interval '24 hours',
 UNIQUE(run_id,id), FOREIGN KEY(workspace_id,run_id) REFERENCES product_bulk_runs(workspace_id,id) ON DELETE CASCADE
);
CREATE TABLE product_bulk_review_rows (
 review_id uuid NOT NULL REFERENCES product_bulk_reviews(id) ON DELETE CASCADE,
 row_id uuid NOT NULL REFERENCES product_bulk_rows(id) ON DELETE CASCADE, ordinal bigint NOT NULL,
 request jsonb NOT NULL, material jsonb NOT NULL, state text NOT NULL DEFAULT 'pending', receipt jsonb,
 PRIMARY KEY(review_id,row_id)
);
CREATE INDEX product_bulk_review_rows_order ON product_bulk_review_rows(review_id,ordinal) WHERE state='pending';
CREATE TABLE product_bulk_settings (
 workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
 per_run_cap_usd numeric(18,6) NOT NULL DEFAULT 0 CHECK(per_run_cap_usd>=0), updated_by uuid REFERENCES users(id), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE product_bulk_ai_costs (
 id uuid PRIMARY KEY DEFAULT uuidv7(), workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 run_id uuid NOT NULL, source_id uuid, request_key text NOT NULL, upper_bound_usd numeric(18,6) NOT NULL CHECK(upper_bound_usd>=0),
 actual_usd numeric(18,6) CHECK(actual_usd>=0), state text NOT NULL DEFAULT 'reserved' CHECK(state IN ('reserved','settled')),
 usage jsonb, created_at timestamptz NOT NULL DEFAULT now(), settled_at timestamptz,
 UNIQUE(workspace_id,request_key), FOREIGN KEY(workspace_id,run_id) REFERENCES product_bulk_runs(workspace_id,id) ON DELETE CASCADE
);
CREATE INDEX product_bulk_ai_costs_month ON product_bulk_ai_costs(workspace_id,created_at);
CREATE INDEX product_bulk_ai_costs_source ON product_bulk_ai_costs(run_id,source_id);
CREATE TABLE product_bulk_mappings (
 id uuid PRIMARY KEY DEFAULT uuidv7(), workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 name text NOT NULL, mapping jsonb NOT NULL, created_by uuid NOT NULL REFERENCES users(id), created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(workspace_id,name)
);
