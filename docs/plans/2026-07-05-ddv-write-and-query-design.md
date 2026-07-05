# ddv — add Write + Query (design)

Date: 2026-07-05
Status: accepted, pre-implementation
Fork: local only (`~/Development/ddv`, branch `feat/write-and-query`, remote `upstream` = lusingander/ddv). No GitHub fork.

## Goal
Turn ddv (a read-only DynamoDB TUI) into one that can also **query** (targeted, no
full scans) and **edit / create / delete** items — a DynamoDB TUI that's pleasant
to browse *and* mutate.

## Scope (agreed)
- **Query**: guided form (index picker + PK + optional SK condition) with an
  advanced raw-expression escape hatch.
- **Edit / Create / Delete**: embedded multi-line JSON editor with a typed⇄plain
  JSON toggle; delete via confirm.
- Out of scope for v1 (YAGNI): duplicate/clone, per-attribute inline editing,
  transactions, batch edits.

## Architecture fit
ddv is ratatui + `aws-sdk-dynamodb`, structured as an `App` holding a `ViewStack`
of `View`s (`Init/TableList/Table/Item/TableInsight/Help`). Keys → `UserEvent`
(semantic) via config; side effects → `AppEvent` dispatched on tokio, results sent
back through a channel (`tx`) as `Complete*` events feeding the `Notify*` status
bar. New work plugs into these same three seams.

### Client (`src/client.rs`)
- `query_items(table, index: Option<&str>, key_cond, filter, limit)` → Query,
  building `KeyConditionExpression` (PK `=`, optional SK `= | begins_with | < | <= |
  > | >= | between`); targets a GSI/LSI when `index` is set. Paginated via
  `LastEvaluatedKey`.
- `put_item(table, item)` → PutItem (covers edit + create).
- `delete_item(table, key)` → DeleteItem by key.

### Events
New `AppEvent`s: `OpenQueryForm`, `RunQuery`/`CompleteQuery`, `OpenEditor(mode)`,
`SaveItem`/`CompleteSaveItem`, `RequestDelete`/`DeleteItem`/`CompleteDeleteItem`.
New `UserEvent`s: `Query`, `Edit`, `New`, `Delete`, `Save`, `ToggleJsonMode`,
`ToggleAdvancedQuery`.

## Query UI — `QueryView`
Pushed from the Table view on `?` (`/` stays client-side QuickFilter). Guided form:
- **Index** cycle field: `(base table)` + each GSI/LSI (from `describe_table`);
  selecting relabels key fields with that index's real attribute names.
- **Partition key**: `tui-input`, always `=`, labeled with the real PK attr.
- **Sort key condition** (only if the index has a sort key): operator cycle
  (`= / begins_with / < / <= / > / >= / between`) + value input(s).
Navigation: Up/Down between fields, Left/Right to cycle, Confirm runs, Close
cancels. `CompleteQuery` pops back to the Table view showing results (a "QUERY"
indicator vs scan); `next-page` paginates. `Reset`/`Reload` returns to scan.
**Advanced toggle** (`Ctrl-a`): single raw-expression input,
`PK = "x" and SK ^= "pre" using index("orgIndex")`, parsed by a small bounded
parser (PK cond [+ SK cond] [+ using index]); builds the same `query_items` call.

## Edit / Create / Delete — `EditView`
Adds `tui-textarea`. Pushed on the stack; multi-line JSON editor.
- **Edit** (`e`): load selected item's JSON. Save (`Ctrl-s`): parse → attributes →
  validate key attrs present → `put_item`. Success pops + refreshes; parse error
  keeps the buffer + shows message.
- **Create** (`n`): editor pre-filled with a skeleton from the key schema.
- **Delete** (`d`): confirm dialog showing the key → `delete_item` → refresh.
- **JSON mode toggle** (`Ctrl-t`): re-serialize buffer between **typed**
  (`{"age":{"N":"30"}}`) and **plain** (`{"age":30}`). Save infers types in plain
  mode. Constraints: plain JSON can't express **sets/binary** → toggling such an
  item warns and stays typed; plain numbers parsed as **arbitrary-precision raw
  tokens** → no float mangling of big ints/decimals. Invalid JSON blocks the toggle
  with an error.

## Safety & errors
- `--read-only` CLI flag disables all write actions with a status hint (wired to
  the `dev-permissions` launch).
- Any write/delete against a **non-local** endpoint shows a confirm dialog with
  table + key.
- Editor title states **editing** (existing key) vs **creating** (PutItem
  overwrites).
- AWS errors → existing `NotifyError` status bar.

## Testing
`rstest` (already a dev-dep) unit tests:
- advanced-expression parser,
- typed⇄plain JSON conversion (round-trip both directions),
- key-condition builder,
- create-skeleton generator.
End-to-end against the **local DynamoDB** container (`--endpoint-url
http://localhost:8000`): query → edit → save → delete on a throwaway table.

## Implementation order (vertical slices)
1. `client.query_items` + `QueryView` (guided form) + wire events → query works.
2. Advanced-expression parser + toggle.
3. `client.put_item` + `EditView` (typed JSON) + edit/create + save.
4. Typed⇄plain toggle.
5. `client.delete_item` + confirm.
6. `--read-only` + non-local write confirm.
7. Tests + local e2e.
