-- Product-only typed attributes; ordinary product transactions own every value change.
CREATE TABLE product_field_definitions (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    key text NOT NULL CHECK (char_length(key) BETWEEN 1 AND 100),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 255),
    type text NOT NULL CHECK (type IN ('text','number','boolean','date','select','multiselect')),
    status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft','active','archived')),
    required boolean NOT NULL DEFAULT false,
    nullable boolean NOT NULL DEFAULT true,
    max_length integer NOT NULL DEFAULT 16384 CHECK (max_length BETWEEN 1 AND 16384),
    min numeric(24,8), max numeric(24,8),
    options jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(options)='array' AND jsonb_array_length(options)<=200),
    used boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (min IS NULL OR max IS NULL OR min<=max),
    UNIQUE(workspace_id,id), UNIQUE(workspace_id,id,type), UNIQUE(workspace_id,key)
);
CREATE TABLE product_field_values (
    workspace_id uuid NOT NULL,
    product_id uuid NOT NULL,
    field_id uuid NOT NULL,
    type text NOT NULL,
    value_text text, value_number numeric(24,8), value_boolean boolean, value_date date,
    value_option uuid, value_options uuid[],
    PRIMARY KEY(workspace_id,product_id,field_id),
    FOREIGN KEY(workspace_id,product_id) REFERENCES products(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id,field_id,type) REFERENCES product_field_definitions(workspace_id,id,type),
    CHECK (num_nonnulls(value_text,value_number,value_boolean,value_date,value_option,value_options)=1),
    CHECK ((type='text')=(value_text IS NOT NULL)),
    CHECK ((type='number')=(value_number IS NOT NULL)),
    CHECK ((type='boolean')=(value_boolean IS NOT NULL)),
    CHECK ((type='date')=(value_date IS NOT NULL)),
    CHECK ((type='select')=(value_option IS NOT NULL)),
    CHECK ((type='multiselect')=(value_options IS NOT NULL)),
    CHECK (value_text IS NULL OR char_length(value_text)<=16384),
    CHECK (value_options IS NULL OR cardinality(value_options)<=200)
);
CREATE INDEX product_field_text_filter ON product_field_values(workspace_id,field_id,md5(value_text),product_id) WHERE type='text';
CREATE INDEX product_field_number_filter ON product_field_values(workspace_id,field_id,value_number,product_id) WHERE type='number';
CREATE INDEX product_field_boolean_filter ON product_field_values(workspace_id,field_id,value_boolean,product_id) WHERE type='boolean';
CREATE INDEX product_field_date_filter ON product_field_values(workspace_id,field_id,value_date,product_id) WHERE type='date';
CREATE INDEX product_field_select_filter ON product_field_values(workspace_id,field_id,value_option,product_id) WHERE type='select';
CREATE INDEX product_field_multiselect_filter ON product_field_values USING gin(value_options) WHERE type='multiselect';
CREATE INDEX product_field_definition_values ON product_field_values(workspace_id,field_id,product_id);
CREATE INDEX product_field_text_contains ON product_field_values USING gin(value_text gin_trgm_ops) WHERE type='text';
