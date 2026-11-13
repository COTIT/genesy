# Product custom fields

Product-only definitions live under `/v1/workspace/{workspace}/product-fields`.
Reading requires `product.view` or `product.list`. Creating, editing and archiving
requires workspace admin. Another workspace's definition is 404.

- `GET /`: `{data:[definition,...]}`, ordered by UUID. Optional `status=draft|active|archived`.
- `POST /`: 201 `{data:definition}`.
- `GET /{field}`: `{data:definition}`.
- `PATCH /{field}`: `{data:definition}`; omitted properties stay unchanged.
- `DELETE /{field}`: archive, 200 `{data:definition}`. Values remain readable.

A definition contains `id`, immutable case-sensitive `key`, `label`, `type`,
`status`, `required`, `nullable`, `max_length`, `min`, `max`, `options`, `used`,
`revision`, `created_at` and `updated_at`. Keys use 1–100 ASCII letters, digits,
underscores or hyphens; labels use 1–255 characters. `Color` and `color` are
different keys. Keys remain reserved when archived.

Create requires `key`, `label` and `type`. Defaults: `status:"draft"`,
`required:false`, `nullable:true`, `max_length:16384`, no numeric bounds/options.
The six wire types are `text`, `number`, `boolean`, `date`, `select`, `multiselect`.

Text supports `max_length` from 1 through 16384. Number supports exact decimal
string `min`/`max` bounds; explicit null clears a bound. Values fit numeric(24,8),
with no rounding. Select types require 1–200 options `{id:UUID,label:string}`.
Supply stable option UUIDs when creating or replacing the option list. Existing
values prevent removing used options or narrowing constraints past stored values.
A field type cannot change once a value has ever been written, including after
all its values were cleared.

Draft fields accept values for backfill. Required rules apply only while active.
Activating a required definition succeeds only when every live product has a
value (a nonempty selection for multiselect); 422 `errors.required` reports the
number of missing products. This check and product writes share the workspace
catalog identity lock. Archived definitions cannot change or receive assignments.

## Native product values

Native product create/update accepts `custom_fields`, an object keyed by field
UUID. For example:

```json
{
  "name":"Tea",
  "price":"12.50",
  "custom_fields":{
    "019b0000-0000-7000-8000-000000000001":"Tanzania",
    "019b0000-0000-7000-8000-000000000002":"12.5000"
  }
}
```

Types: text = nonempty string; number = exact decimal string; boolean = JSON
true/false; date = `YYYY-MM-DD`; select = option UUID string; multiselect = array
of distinct option UUIDs. Unknown or foreign
fields, duplicate UUID spellings, invalid options and values return 422 under
`custom_fields.<uuid>`. No arbitrary scalar coercion. Dates are calendar-checked.
Omitted fields stay; explicit null clears only nullable, nonrequired fields.
An empty multiselect is allowed unless the field is active and required.

Product list/detail resources return the `custom_fields` map. Decimal strings are
canonical without redundant zeroes. Field writes, product prices, variant price
inheritance, stock seeding, price history and events share the native transaction.
Duplicate copies all field values, including archived definitions. Deleting a
product retains values with its soft-deleted product.

## Filtering

`GET /products?custom_filters=<URL-encoded JSON array>` combines at most 20 filters
with AND, alongside native product filters. Each filter has `field_id`,
`operator`, and `value`:

```json
[{"field_id":"019b0000-0000-7000-8000-000000000002","operator":"gte","value":"10"}]
```

- `eq`: text (case-sensitive), number, boolean, date, select.
- `contains`: text (case-insensitive literal substring; `%` and `_` stay literal),
  or multiselect (one option UUID).
- `any`: multiselect, an array of option UUIDs; matches any selected option.
- `lt`, `lte`, `gt`, `gte`: number/date.
- `is_set`: all types; boolean value, default true. Empty optional multiselect
  counts as set. Null/absent values count as unset.

Filters validate field ownership/types before querying. Archived field values
remain filterable. Scalar typed B-tree indexes, multiselect GIN and text trigram
GIN support predicates. Text equality indexes a digest and verifies full equality,
so 16 KiB text cannot exceed a B-tree index entry limit. No JSON-to-number casts
occur in value filter predicates.

## Internal transaction contract

`products::service::{create_in,update_in,delete_in}` accept the caller's transaction;
normal HTTP wrappers commit it. `lock_identity` serializes catalog matching with
native mutations/definition edits. `snapshot_in` and `material_in` provide private
complete before/projected values and dependency data for durable import review.
They are not public product resources. Create projections exclude fresh timestamps
and allocated product numbers; field usage markers are excluded from dependency
material so earlier successful import rows do not invalidate later rows.
