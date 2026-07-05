use std::{
    collections::{BTreeSet, HashMap},
    str::FromStr,
    time::Duration,
};

use aws_config::{default_provider, meta::region::RegionProviderChain, BehaviorVersion, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition as AwsAttributeDefinition, AttributeValue as AwsAttributeValue,
    GlobalSecondaryIndexDescription as AwsGlobalSecondaryIndexDescription,
    KeySchemaElement as AwsKeySchemaElement, KeyType as AwsKeyType,
    LocalSecondaryIndexDescription as AwsLocalSecondaryIndexDescription,
    Projection as AwsProjection, ProjectionType as AwsProjectionType,
    ProvisionedThroughputDescription as AwsProvisionedThroughputDescription,
    ScalarAttributeType as AwsScalarAttributeType, TableDescription as AwsTableDescription,
    TableStatus as AwsTableStatus,
};
use aws_sdk_dynamodb::primitives::Blob;
use aws_smithy_types::{timeout::TimeoutConfig, DateTime as AwsDateTime};
use chrono::{DateTime, Local, TimeZone as _};
use rust_decimal::Decimal;

use crate::{
    data::{
        Attribute, AttributeDefinition, GlobalSecondaryIndexDescription, Item, KeySchemaElement,
        KeySchemaType, KeyType, LocalSecondaryIndexDescription, Projection, ProjectionType,
        ProvisionedThroughput, QueryRequest, ScalarAttributeType, SortKeyCondition, Table,
        TableDescription, TableStatus,
    },
    error::{AppError, AppResult},
};

/// How many items to pull when a table is first opened. Bounded on purpose:
/// opening a table must never trigger a full-table scan. Use a query for more.
pub const DEFAULT_SCAN_LIMIT: i32 = 100;

pub struct Client {
    client: aws_sdk_dynamodb::Client,
}

impl Client {
    pub async fn new(
        region: Option<String>,
        endpoint_url: Option<String>,
        profile: Option<String>,
        default_region_fallback: String,
    ) -> Client {
        let mut region_builder = default_provider::region::Builder::default();
        if let Some(profile) = &profile {
            region_builder = region_builder.profile_name(profile);
        }
        let region_provider = RegionProviderChain::first_try(region.map(Region::new))
            .or_else(region_builder.build())
            .or_else(Region::new(default_region_fallback));

        // Fail fast on an unreachable endpoint (e.g. a stopped local DynamoDB)
        // instead of hanging on the loading screen forever.
        let timeout_config = TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(3))
            .operation_timeout(Duration::from_secs(20))
            .build();
        let mut config_loader = aws_config::defaults(BehaviorVersion::latest())
            .region(region_provider)
            .timeout_config(timeout_config);
        if let Some(endpoint_url) = &endpoint_url {
            config_loader = config_loader.endpoint_url(endpoint_url);
        }
        if let Some(profile) = &profile {
            config_loader = config_loader.profile_name(profile);
        }
        let sdk_config = config_loader.load().await;

        let config_builder = aws_sdk_dynamodb::config::Builder::from(&sdk_config);
        let config = config_builder.build();

        let client = aws_sdk_dynamodb::Client::from_conf(config);
        Client { client }
    }

    pub async fn list_all_tables(&self) -> AppResult<Vec<Table>> {
        let mut last_evaluated_table_name = None;
        let mut tables = Vec::new();
        loop {
            let mut req = self.client.list_tables();
            if let Some(table_name) = last_evaluated_table_name {
                req = req.exclusive_start_table_name(table_name);
            }

            let result = req.send().await;
            let output = result.map_err(|e| AppError::new("failed to list tables", e))?;

            tables.extend(
                output
                    .table_names
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into),
            );

            if output.last_evaluated_table_name.is_none() {
                break;
            }
            last_evaluated_table_name = output.last_evaluated_table_name;
        }
        Ok(tables)
    }

    pub async fn describe_table(&self, table_name: &str) -> AppResult<TableDescription> {
        let req = self.client.describe_table().table_name(table_name);

        let result = req.send().await;
        let output = result.map_err(|e| AppError::new("failed to load table description", e))?;

        let desc = to_table_description(output.table.unwrap());
        Ok(desc)
    }

    /// Load the first bounded page of a table. Deliberately does NOT drain the
    /// whole table — a full scan of a large table is exactly what we avoid.
    pub async fn scan_items(
        &self,
        table_name: &str,
        schema: &KeySchemaType,
        limit: i32,
    ) -> AppResult<Vec<Item>> {
        let output = self
            .client
            .scan()
            .table_name(table_name)
            .limit(limit)
            .send()
            .await
            .map_err(|e| AppError::new("failed to scan items", e))?;

        let mut items: Vec<Item> = output
            .items
            .unwrap_or_default()
            .into_iter()
            .map(to_item)
            .collect();
        sort_items(&mut items, schema);
        Ok(items)
    }

    pub async fn query_items(
        &self,
        table_name: &str,
        request: &QueryRequest,
        schema: &KeySchemaType,
    ) -> AppResult<Vec<Item>> {
        let (key_condition, names, values) = build_key_condition(request);

        let mut last_evaluated_key = None;
        let mut items = Vec::new();
        loop {
            let mut req = self
                .client
                .query()
                .table_name(table_name)
                .key_condition_expression(&key_condition)
                .set_expression_attribute_names(Some(names.clone()))
                .set_expression_attribute_values(Some(values.clone()));
            if let Some(index) = &request.index_name {
                req = req.index_name(index);
            }
            if last_evaluated_key.is_some() {
                req = req.set_exclusive_start_key(last_evaluated_key);
            }

            let result = req.send().await;
            let output = result.map_err(|e| AppError::new("failed to query items", e))?;

            items.extend(output.items.unwrap_or_default().into_iter().map(to_item));

            if output.last_evaluated_key.is_none() {
                break;
            }
            last_evaluated_key = output.last_evaluated_key;
        }
        sort_items(&mut items, schema);
        Ok(items)
    }

    pub async fn put_item(&self, table_name: &str, item: &Item) -> AppResult<()> {
        let aws_item = to_aws_item(item);
        self.client
            .put_item()
            .table_name(table_name)
            .set_item(Some(aws_item))
            .send()
            .await
            .map_err(|e| AppError::new("failed to put item", e))?;
        Ok(())
    }

    pub async fn delete_item(
        &self,
        table_name: &str,
        item: &Item,
        schema: &KeySchemaType,
    ) -> AppResult<()> {
        let key = key_of(item, schema);
        self.client
            .delete_item()
            .table_name(table_name)
            .set_key(Some(key))
            .send()
            .await
            .map_err(|e| AppError::new("failed to delete item", e))?;
        Ok(())
    }
}

/// Build a `KeyConditionExpression` plus its expression attribute name/value maps
/// from a [`QueryRequest`]. Uses `#p`/`#s` name placeholders and `:p`/`:s`/`:s2`
/// value placeholders so arbitrary attribute names are safe.
fn build_key_condition(
    request: &QueryRequest,
) -> (
    String,
    HashMap<String, String>,
    HashMap<String, AwsAttributeValue>,
) {
    let mut names = HashMap::new();
    let mut values = HashMap::new();

    let (pk_name, pk_value) = &request.partition_key;
    names.insert("#p".to_string(), pk_name.clone());
    values.insert(":p".to_string(), to_aws_attribute_value(pk_value));
    let mut expr = "#p = :p".to_string();

    if let Some((sk_name, cond)) = &request.sort_key {
        names.insert("#s".to_string(), sk_name.clone());
        match cond {
            SortKeyCondition::Eq(v) => {
                values.insert(":s".to_string(), to_aws_attribute_value(v));
                expr.push_str(" AND #s = :s");
            }
            SortKeyCondition::BeginsWith(s) => {
                values.insert(":s".to_string(), AwsAttributeValue::S(s.clone()));
                expr.push_str(" AND begins_with(#s, :s)");
            }
            SortKeyCondition::Lt(v) => {
                values.insert(":s".to_string(), to_aws_attribute_value(v));
                expr.push_str(" AND #s < :s");
            }
            SortKeyCondition::Le(v) => {
                values.insert(":s".to_string(), to_aws_attribute_value(v));
                expr.push_str(" AND #s <= :s");
            }
            SortKeyCondition::Gt(v) => {
                values.insert(":s".to_string(), to_aws_attribute_value(v));
                expr.push_str(" AND #s > :s");
            }
            SortKeyCondition::Ge(v) => {
                values.insert(":s".to_string(), to_aws_attribute_value(v));
                expr.push_str(" AND #s >= :s");
            }
            SortKeyCondition::Between(a, b) => {
                values.insert(":s".to_string(), to_aws_attribute_value(a));
                values.insert(":s2".to_string(), to_aws_attribute_value(b));
                expr.push_str(" AND #s BETWEEN :s AND :s2");
            }
        }
    }

    (expr, names, values)
}

fn key_of(item: &Item, schema: &KeySchemaType) -> HashMap<String, AwsAttributeValue> {
    let mut key = HashMap::new();
    let mut add = |name: &str| {
        if let Some(attr) = item.attributes.get(name) {
            key.insert(name.to_string(), to_aws_attribute_value(attr));
        }
    };
    match schema {
        KeySchemaType::Hash(pk) => add(pk),
        KeySchemaType::HashRange(pk, sk) => {
            add(pk);
            add(sk);
        }
    }
    key
}

pub fn to_aws_item(item: &Item) -> HashMap<String, AwsAttributeValue> {
    item.attributes
        .iter()
        .map(|(k, v)| (k.clone(), to_aws_attribute_value(v)))
        .collect()
}

fn to_aws_attribute_value(attr: &Attribute) -> AwsAttributeValue {
    match attr {
        Attribute::S(s) => AwsAttributeValue::S(s.clone()),
        Attribute::N(n) => AwsAttributeValue::N(n.to_string()),
        Attribute::B(b) => AwsAttributeValue::B(Blob::new(b.clone())),
        Attribute::BOOL(b) => AwsAttributeValue::Bool(*b),
        Attribute::NULL => AwsAttributeValue::Null(true),
        Attribute::L(vs) => AwsAttributeValue::L(vs.iter().map(to_aws_attribute_value).collect()),
        Attribute::M(m) => AwsAttributeValue::M(
            m.iter()
                .map(|(k, v)| (k.clone(), to_aws_attribute_value(v)))
                .collect(),
        ),
        Attribute::SS(ss) => AwsAttributeValue::Ss(ss.iter().cloned().collect()),
        Attribute::NS(ns) => AwsAttributeValue::Ns(ns.iter().map(|n| n.to_string()).collect()),
        Attribute::BS(bs) => {
            AwsAttributeValue::Bs(bs.iter().map(|b| Blob::new(b.clone())).collect())
        }
    }
}

impl From<String> for Table {
    fn from(name: String) -> Self {
        Table { name }
    }
}

fn to_table_description(desc: AwsTableDescription) -> TableDescription {
    let attribute_definitions = vec_into(desc.attribute_definitions.unwrap());
    let table_name = desc.table_name.unwrap();
    let key_schema = vec_into(desc.key_schema.unwrap());
    let table_status = desc.table_status.unwrap().into();
    let creation_date_time = convert_datetime(desc.creation_date_time.unwrap());
    let provisioned_throughput = desc.provisioned_throughput.map(Into::into);
    let total_size_bytes = desc.table_size_bytes.unwrap() as u64;
    let item_count = desc.item_count.unwrap() as u64;
    let table_arn = desc.table_arn.unwrap();
    let local_secondary_indexes = desc.local_secondary_indexes.map(vec_into);
    let global_secondary_indexes = desc.global_secondary_indexes.map(vec_into);

    let key_schema_type = to_key_schema_type(key_schema.clone());

    TableDescription {
        attribute_definitions,
        table_name,
        key_schema,
        table_status,
        creation_date_time,
        provisioned_throughput,
        total_size_bytes,
        item_count,
        table_arn,
        local_secondary_indexes,
        global_secondary_indexes,

        key_schema_type,
    }
}

impl From<AwsAttributeDefinition> for AttributeDefinition {
    fn from(def: AwsAttributeDefinition) -> Self {
        AttributeDefinition::new(def.attribute_name, def.attribute_type.into())
    }
}

impl From<AwsScalarAttributeType> for ScalarAttributeType {
    fn from(s: AwsScalarAttributeType) -> Self {
        match s {
            AwsScalarAttributeType::B => ScalarAttributeType::B,
            AwsScalarAttributeType::N => ScalarAttributeType::N,
            AwsScalarAttributeType::S => ScalarAttributeType::S,
            _ => unreachable!("unexpected scalar attribute type: {:?}", s),
        }
    }
}

impl From<AwsTableStatus> for TableStatus {
    fn from(s: AwsTableStatus) -> Self {
        match s {
            AwsTableStatus::Active => TableStatus::Active,
            AwsTableStatus::Archived => TableStatus::Archived,
            AwsTableStatus::Archiving => TableStatus::Archiving,
            AwsTableStatus::Creating => TableStatus::Creating,
            AwsTableStatus::Deleting => TableStatus::Deleting,
            AwsTableStatus::InaccessibleEncryptionCredentials => {
                TableStatus::InaccessibleEncryptionCredentials
            }
            AwsTableStatus::Updating => TableStatus::Updating,
            _ => unreachable!("unexpected table status: {:?}", s),
        }
    }
}

fn to_key_schema(key_schema: Vec<AwsKeySchemaElement>) -> Vec<KeySchemaElement> {
    key_schema.into_iter().map(Into::into).collect()
}

impl From<AwsKeySchemaElement> for KeySchemaElement {
    fn from(schema: AwsKeySchemaElement) -> Self {
        KeySchemaElement {
            attribute_name: schema.attribute_name,
            key_type: schema.key_type.into(),
        }
    }
}

impl From<AwsKeyType> for KeyType {
    fn from(t: AwsKeyType) -> Self {
        match t {
            AwsKeyType::Hash => KeyType::Hash,
            AwsKeyType::Range => KeyType::Range,
            _ => unreachable!("unexpected key type: {:?}", t),
        }
    }
}

impl From<AwsLocalSecondaryIndexDescription> for LocalSecondaryIndexDescription {
    fn from(value: AwsLocalSecondaryIndexDescription) -> Self {
        let index_name = value.index_name.unwrap();
        let key_schema = to_key_schema(value.key_schema.unwrap());
        let projection = value.projection.unwrap().into();
        let index_size_bytes = value.index_size_bytes.unwrap_or(0) as u64;
        let item_count = value.item_count.unwrap_or(0) as u64;
        let index_arn = value.index_arn.unwrap_or("".to_string());
        LocalSecondaryIndexDescription {
            index_name,
            key_schema,
            projection,
            index_size_bytes,
            item_count,
            index_arn,
        }
    }
}

impl From<AwsGlobalSecondaryIndexDescription> for GlobalSecondaryIndexDescription {
    fn from(value: AwsGlobalSecondaryIndexDescription) -> Self {
        let index_name = value.index_name.unwrap();
        let key_schema = to_key_schema(value.key_schema.unwrap());
        let projection = value.projection.unwrap().into();
        let index_size_bytes = value.index_size_bytes.unwrap() as u64;
        let item_count = value.item_count.unwrap() as u64;
        let index_arn = value.index_arn.unwrap();
        GlobalSecondaryIndexDescription {
            index_name,
            key_schema,
            projection,
            index_size_bytes,
            item_count,
            index_arn,
        }
    }
}

impl From<AwsProjection> for Projection {
    fn from(p: AwsProjection) -> Self {
        let projection_type = p.projection_type.unwrap().into();
        let non_key_attributes = p.non_key_attributes;
        Projection {
            projection_type,
            non_key_attributes,
        }
    }
}

impl From<AwsProjectionType> for ProjectionType {
    fn from(t: AwsProjectionType) -> Self {
        match t {
            AwsProjectionType::All => ProjectionType::All,
            AwsProjectionType::KeysOnly => ProjectionType::KeysOnly,
            AwsProjectionType::Include => ProjectionType::Include,
            _ => unreachable!("unexpected projection type: {:?}", t),
        }
    }
}

fn to_key_schema_type(elements: Vec<KeySchemaElement>) -> KeySchemaType {
    let mut hash_key = None;
    let mut range_key = None;
    for elem in elements {
        match elem.key_type {
            KeyType::Hash => {
                if hash_key.is_some() {
                    panic!("multiple hash keys");
                }
                hash_key = Some(elem.attribute_name);
            }
            KeyType::Range => {
                if range_key.is_some() {
                    panic!("multiple range keys");
                }
                range_key = Some(elem.attribute_name);
            }
        }
    }
    match (hash_key, range_key) {
        (Some(hash_key), Some(range_key)) => KeySchemaType::HashRange(hash_key, range_key),
        (Some(hash_key), None) => KeySchemaType::Hash(hash_key),
        (hash_key, range_key) => {
            panic!("unexpected key schema: ({hash_key:?}, {range_key:?})")
        }
    }
}

fn to_item(attributes: HashMap<String, AwsAttributeValue>) -> Item {
    let attributes = attributes.into_iter().map(|(k, v)| (k, v.into())).collect();
    Item { attributes }
}

impl From<AwsAttributeValue> for Attribute {
    fn from(value: AwsAttributeValue) -> Self {
        match value {
            AwsAttributeValue::S(s) => Attribute::S(s),
            AwsAttributeValue::N(n) => Attribute::N(Decimal::from_str(&n).unwrap()),
            AwsAttributeValue::B(b) => Attribute::B(b.into_inner()),
            AwsAttributeValue::Bool(b) => Attribute::BOOL(b),
            AwsAttributeValue::Null(_) => Attribute::NULL,
            AwsAttributeValue::M(m) => {
                let m = m.into_iter().map(|(k, v)| (k, v.into())).collect();
                Attribute::M(m)
            }
            AwsAttributeValue::L(vs) => {
                let vs = vs.into_iter().map(Into::into).collect();
                Attribute::L(vs)
            }
            AwsAttributeValue::Ss(ss) => {
                let ss = BTreeSet::from_iter(ss);
                Attribute::SS(ss)
            }
            AwsAttributeValue::Ns(ns) => {
                let ns =
                    BTreeSet::from_iter(ns.into_iter().map(|n| Decimal::from_str(&n).unwrap()));
                Attribute::NS(ns)
            }
            AwsAttributeValue::Bs(bs) => {
                let bs = BTreeSet::from_iter(bs.into_iter().map(|b| b.into_inner()));
                Attribute::BS(bs)
            }
            _ => unreachable!("unexpected attribute value: {:?}", value),
        }
    }
}

impl From<AwsProvisionedThroughputDescription> for ProvisionedThroughput {
    fn from(t: AwsProvisionedThroughputDescription) -> Self {
        ProvisionedThroughput {
            last_increase_date_time: t.last_increase_date_time.map(convert_datetime),
            last_decrease_date_time: t.last_decrease_date_time.map(convert_datetime),
            number_of_decreases_today: t.number_of_decreases_today.unwrap() as u64,
            read_capacity_units: t.read_capacity_units.unwrap() as u64,
            write_capacity_units: t.write_capacity_units.unwrap() as u64,
        }
    }
}

fn sort_items(items: &mut [Item], schema: &KeySchemaType) {
    match schema {
        KeySchemaType::Hash(hash_key) => {
            items.sort_by(|a, b| {
                let a = a.attributes.get(hash_key).unwrap();
                let b = b.attributes.get(hash_key).unwrap();
                a.partial_cmp(b).unwrap()
            });
        }
        KeySchemaType::HashRange(hash_key, range_key) => {
            items.sort_by(|a, b| {
                let a_hash = a.attributes.get(hash_key).unwrap();
                let b_hash = b.attributes.get(hash_key).unwrap();
                match a_hash.partial_cmp(b_hash).unwrap() {
                    std::cmp::Ordering::Equal => {
                        let a_range = a.attributes.get(range_key).unwrap();
                        let b_range = b.attributes.get(range_key).unwrap();
                        a_range.partial_cmp(b_range).unwrap()
                    }
                    ord => ord,
                }
            });
        }
    }
}

fn convert_datetime(dt: AwsDateTime) -> DateTime<Local> {
    let nanos = dt.as_nanos();
    Local.timestamp_nanos(nanos as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &AwsAttributeValue) -> &str {
        match v {
            AwsAttributeValue::S(s) => s,
            other => panic!("expected S, got {other:?}"),
        }
    }

    #[test]
    fn key_condition_pk_only() {
        let req = QueryRequest {
            index_name: None,
            partition_key: ("PK".into(), Attribute::S("u#1".into())),
            sort_key: None,
        };
        let (expr, names, values) = build_key_condition(&req);
        assert_eq!(expr, "#p = :p");
        assert_eq!(names.get("#p").map(String::as_str), Some("PK"));
        assert_eq!(s(values.get(":p").unwrap()), "u#1");
    }

    #[test]
    fn key_condition_pk_and_begins_with() {
        let req = QueryRequest {
            index_name: Some("orgIndex".into()),
            partition_key: ("orgId".into(), Attribute::S("o#7".into())),
            sort_key: Some(("SK".into(), SortKeyCondition::BeginsWith("PAY#".into()))),
        };
        let (expr, names, values) = build_key_condition(&req);
        assert_eq!(expr, "#p = :p AND begins_with(#s, :s)");
        assert_eq!(names.get("#s").map(String::as_str), Some("SK"));
        assert_eq!(s(values.get(":s").unwrap()), "PAY#");
    }

    #[test]
    fn key_condition_between() {
        let req = QueryRequest {
            index_name: None,
            partition_key: ("PK".into(), Attribute::S("x".into())),
            sort_key: Some((
                "SK".into(),
                SortKeyCondition::Between(
                    Attribute::S("2024-01".into()),
                    Attribute::S("2024-12".into()),
                ),
            )),
        };
        let (expr, _names, values) = build_key_condition(&req);
        assert_eq!(expr, "#p = :p AND #s BETWEEN :s AND :s2");
        assert_eq!(s(values.get(":s").unwrap()), "2024-01");
        assert_eq!(s(values.get(":s2").unwrap()), "2024-12");
    }
}

fn vec_into<T, U>(ts: Vec<T>) -> Vec<U>
where
    U: From<T>,
{
    ts.into_iter().map(Into::into).collect()
}
