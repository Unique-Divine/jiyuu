//! File-local Kubernetes declarations in Terraform and OpenTofu modules.
//!
//! The caller retains the parsed HCL tree and publishes the records through
//! the existing Kubernetes identity and graph writer.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::slice;

// Layouts from the vendored tree_sitter/api.h. Nodes borrow the caller's tree.
#[derive(Clone, Copy)]
#[repr(C)]
struct TsNode {
    context: [u32; 4],
    id: *const c_void,
    tree: *const c_void,
}

#[repr(C)]
struct TsPoint {
    row: u32,
    column: u32,
}

unsafe extern "C" {
    fn ts_node_type(node: TsNode) -> *const c_char;
    fn ts_node_named_child_count(node: TsNode) -> u32;
    fn ts_node_named_child(node: TsNode, index: u32) -> TsNode;
    fn ts_node_child_by_field_name(
        node: TsNode,
        name: *const c_char,
        len: u32,
    ) -> TsNode;
    fn ts_node_start_byte(node: TsNode) -> u32;
    fn ts_node_end_byte(node: TsNode) -> u32;
    fn ts_node_start_point(node: TsNode) -> TsPoint;
    fn ts_node_end_point(node: TsNode) -> TsPoint;
}

impl TsNode {
    fn is(self, kind: &CStr) -> bool {
        !self.id.is_null()
            && unsafe { CStr::from_ptr(ts_node_type(self)) == kind }
    }

    fn children(self) -> impl Iterator<Item = Self> {
        let count = if self.id.is_null() {
            0
        } else {
            unsafe { ts_node_named_child_count(self) }
        };
        (0..count).map(move |index| unsafe { ts_node_named_child(self, index) })
    }

    fn field(self, name: &CStr) -> Self {
        unsafe {
            ts_node_child_by_field_name(
                self,
                name.as_ptr(),
                name.count_bytes() as u32,
            )
        }
    }

    fn text(self, source: &[u8]) -> &[u8] {
        let start = unsafe { ts_node_start_byte(self) } as usize;
        let end = unsafe { ts_node_end_byte(self) } as usize;
        &source[start..end]
    }
}

fn block_label(node: TsNode, source: &[u8]) -> Option<&[u8]> {
    if node.is(c"identifier") {
        return Some(node.text(source));
    }
    if node.is(c"string_lit") {
        return node
            .children()
            .find(|child| child.is(c"template_literal"))
            .map(|child| child.text(source));
    }
    None
}

// Retain raw HCL literal bytes and the C extractor's field size limits.
fn literal(mut node: TsNode, source: &[u8], capacity: usize) -> Option<Vec<u8>> {
    while node.is(c"expression")
        || node.is(c"literal_value")
        || node.is(c"template_expr")
    {
        let mut children = node.children();
        node = children.next()?;
        if children.next().is_some() {
            return None;
        }
    }
    if !node.is(c"string_lit") && !node.is(c"quoted_template") {
        return None;
    }
    let mut value = Vec::new();
    for child in node.children() {
        if child.is(c"quoted_template_start") || child.is(c"quoted_template_end")
        {
            continue;
        }
        if !child.is(c"template_literal") {
            return None;
        }
        let text = child.text(source);
        if value.len() + text.len() >= capacity {
            return None;
        }
        value.extend_from_slice(text);
    }
    value.truncate(
        value
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(value.len()),
    );
    (!value.contains(&1)).then_some(value)
}

fn attribute(body: TsNode, source: &[u8], key: &[u8]) -> Option<TsNode> {
    body.children()
        .filter(|node| node.is(c"attribute"))
        .find_map(|node| {
            let mut children = node.children();
            (children.next()?.text(source) == key)
                .then(|| children.next())
                .flatten()
        })
}

fn unwrap_first(mut node: TsNode, first: &CStr, second: &CStr) -> TsNode {
    while node.is(first) || node.is(second) {
        let Some(child) = node.children().next() else {
            break;
        };
        node = child;
    }
    node
}

fn object_value(expr: TsNode, source: &[u8], key: &[u8]) -> Option<TsNode> {
    let object = unwrap_first(expr, c"expression", c"collection_value");
    if !object.is(c"object") {
        return None;
    }
    object
        .children()
        .filter(|node| node.is(c"object_elem"))
        .find(|node| {
            let field = node.field(c"key");
            if let Some(text) = literal(field, source, 64) {
                // The native traversal also accepts a quoted empty key.
                return text.is_empty() || text == key;
            }
            let id = unwrap_first(field, c"expression", c"variable_expr");
            id.is(c"identifier") && id.text(source) == key
        })
        .map(|node| node.field(c"val"))
}

fn provider_kind(resource_type: &[u8]) -> Option<(&'static str, &'static str)> {
    Some(match resource_type {
        b"namespace" => ("core", "Namespace"),
        b"service" => ("core", "Service"),
        b"service_account" => ("core", "ServiceAccount"),
        b"default_service_account" => ("core", "ServiceAccount"),
        b"config_map" => ("core", "ConfigMap"),
        b"secret" => ("core", "Secret"),
        b"persistent_volume_claim" => ("core", "PersistentVolumeClaim"),
        b"persistent_volume" => ("core", "PersistentVolume"),
        b"pod" => ("core", "Pod"),
        b"limit_range" => ("core", "LimitRange"),
        b"resource_quota" => ("core", "ResourceQuota"),
        b"endpoints" => ("core", "Endpoints"),
        b"replication_controller" => ("core", "ReplicationController"),
        b"deployment" => ("apps", "Deployment"),
        b"stateful_set" => ("apps", "StatefulSet"),
        b"daemonset" => ("apps", "DaemonSet"),
        b"daemon_set" => ("apps", "DaemonSet"),
        b"job" => ("batch", "Job"),
        b"cron_job" => ("batch", "CronJob"),
        b"cluster_role" => ("rbac.authorization.k8s.io", "ClusterRole"),
        b"cluster_role_binding" => {
            ("rbac.authorization.k8s.io", "ClusterRoleBinding")
        }
        b"role" => ("rbac.authorization.k8s.io", "Role"),
        b"role_binding" => ("rbac.authorization.k8s.io", "RoleBinding"),
        b"network_policy" => ("networking.k8s.io", "NetworkPolicy"),
        b"ingress" => ("networking.k8s.io", "Ingress"),
        b"ingress_class" => ("networking.k8s.io", "IngressClass"),
        b"storage_class" => ("storage.k8s.io", "StorageClass"),
        b"csi_driver" => ("storage.k8s.io", "CSIDriver"),
        b"priority_class" => ("scheduling.k8s.io", "PriorityClass"),
        b"horizontal_pod_autoscaler" => {
            ("autoscaling", "HorizontalPodAutoscaler")
        }
        b"pod_disruption_budget" => ("policy", "PodDisruptionBudget"),
        b"runtime_class" => ("node.k8s.io", "RuntimeClass"),
        b"mutating_webhook_configuration" => (
            "admissionregistration.k8s.io",
            "MutatingWebhookConfiguration",
        ),
        b"validating_webhook_configuration" => (
            "admissionregistration.k8s.io",
            "ValidatingWebhookConfiguration",
        ),
        b"api_service" => ("apiregistration.k8s.io", "APIService"),
        b"certificate_signing_request" => {
            ("certificates.k8s.io", "CertificateSigningRequest")
        }
        _ => return None,
    })
}

struct Object {
    group: Vec<u8>,
    kind: Vec<u8>,
    version: Vec<u8>,
    name: Vec<u8>,
    namespace: Vec<u8>,
}

fn manifest(body: TsNode, source: &[u8]) -> Option<Object> {
    let manifest = attribute(body, source, b"manifest")?;
    let get = |key, capacity| {
        literal(object_value(manifest, source, key)?, source, capacity)
    };
    let metadata = object_value(manifest, source, b"metadata")?;
    let get_metadata =
        |key| literal(object_value(metadata, source, key)?, source, 256);
    let api_version = get(b"apiVersion", 256)?;
    let (group, version) =
        match api_version.iter().position(|&byte| byte == b'/') {
            Some(slash) => (&api_version[..slash], &api_version[slash + 1..]),
            None => (b"core".as_slice(), api_version.as_slice()),
        };
    Some(Object {
        group: group.to_vec(),
        kind: get(b"kind", 128)?,
        version: version[..version.len().min(63)].to_vec(),
        name: get_metadata(b"name")?,
        namespace: get_metadata(b"namespace").unwrap_or_default(),
    })
}

fn declaration(block: TsNode, source: &[u8]) -> Option<(CString, Object)> {
    let mut children = block.children();
    let keyword = children.next()?;
    if !keyword.is(c"identifier") || keyword.text(source) != b"resource" {
        return None;
    }
    let mut block_name = b"resource".to_vec();
    let mut body = None;
    for child in children {
        if child.is(c"body") {
            body = Some(child);
        } else if let Some(label) = block_label(child, source) {
            block_name.push(b'.');
            block_name.extend_from_slice(label);
        }
    }
    let body = body?;
    let labels = block_name.strip_prefix(b"resource.")?;
    let end = labels.iter().position(|&byte| byte == b'.')?;
    let resource_type = labels[..end.min(127)].strip_prefix(b"kubernetes_")?;
    let resource_type =
        match resource_type.iter().rposition(|&byte| byte == b'_') {
            Some(pos)
                if matches!(
                    resource_type.get(pos + 1..pos + 3),
                    Some([b'v', b'0'..=b'9'])
                ) =>
            {
                &resource_type[..pos]
            }
            _ => resource_type,
        };
    let object = if resource_type == b"manifest" {
        manifest(body, source)?
    } else {
        let (group, kind) = provider_kind(resource_type)?;
        let metadata = body
            .children()
            .filter(|child| child.is(c"block"))
            .filter(|child| {
                child
                    .children()
                    .next()
                    .is_some_and(|keyword| keyword.text(source) == b"metadata")
            })
            .find_map(|child| child.children().find(|node| node.is(c"body")));
        let get_metadata =
            |key| literal(attribute(metadata?, source, key)?, source, 256);
        Object {
            group: group.as_bytes().to_vec(),
            kind: kind.as_bytes().to_vec(),
            version: Vec::new(),
            name: if resource_type == b"default_service_account" {
                b"default".to_vec()
            } else {
                get_metadata(b"name")?
            },
            namespace: get_metadata(b"namespace").unwrap_or_default(),
        }
    };
    if object.name.is_empty()
        || object.kind.is_empty()
        || [&object.name, &object.namespace, &object.kind]
            .iter()
            .any(|s| s.contains(&b'/'))
    {
        return None;
    }
    Some((CString::new(block_name).ok()?, object))
}

#[repr(C)]
struct K8sRecord {
    block_name: *const c_char,
    group: *const c_char,
    kind: *const c_char,
    version: *const c_char,
    name: *const c_char,
    namespace_: *const c_char,
    start_line: c_int,
    end_line: c_int,
}

type Publish = unsafe extern "C" fn(*mut c_void, *const K8sRecord);

/// Visit records in top-level HCL block order. The root and source must belong
/// to the same tree and stay live until the call returns. The publisher may
/// read the record and its NUL-terminated strings for the duration of the callback.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_tf_k8s_records(
    root: TsNode,
    source: *const u8,
    len: usize,
    state: *mut c_void,
    publish: Publish,
) {
    let source = unsafe { slice::from_raw_parts(source, len) };
    let Some(body) = root.children().find(|node| node.is(c"body")) else {
        return;
    };
    for block in body.children().filter(|node| node.is(c"block")) {
        let Some((block_name, object)) = declaration(block, source) else {
            continue;
        };
        // Literal values have already been clipped at the first NUL.
        let group = CString::new(object.group).unwrap();
        let kind = CString::new(object.kind).unwrap();
        let version = CString::new(object.version).unwrap();
        let name = CString::new(object.name).unwrap();
        let namespace = CString::new(object.namespace).unwrap();
        let record = K8sRecord {
            block_name: block_name.as_ptr(),
            group: group.as_ptr(),
            kind: kind.as_ptr(),
            version: version.as_ptr(),
            name: name.as_ptr(),
            namespace_: namespace.as_ptr(),
            start_line: unsafe { ts_node_start_point(block) }.row as c_int + 1,
            end_line: unsafe { ts_node_end_point(block) }.row as c_int + 1,
        };
        unsafe { publish(state, &record) };
    }
}

#[cfg(all(test, feature = "c-backend"))]
mod tests {
    use super::*;
    use crate::native_parser::{
        RawLanguage, RawTree, ts_parser_delete, ts_parser_new,
        ts_parser_parse_string, ts_parser_set_language, ts_tree_delete,
    };

    // Link the vendored runtime and pinned HCL grammar. No pipeline state is
    // needed to exercise the same callback the index run will use.
    unsafe extern "C" {
        fn tree_sitter_hcl() -> *const RawLanguage;
        fn ts_tree_root_node(tree: *const RawTree) -> TsNode;
    }

    #[derive(Debug, PartialEq)]
    struct Recorded {
        fields: [String; 6],
        lines: (c_int, c_int),
    }

    unsafe extern "C" fn collect(state: *mut c_void, record: *const K8sRecord) {
        let record = unsafe { &*record };
        let fields = [
            record.block_name,
            record.group,
            record.kind,
            record.version,
            record.name,
            record.namespace_,
        ]
        .map(|value| {
            unsafe { CStr::from_ptr(value) }
                .to_str()
                .unwrap()
                .to_owned()
        });
        unsafe { &mut *state.cast::<Vec<Recorded>>() }.push(Recorded {
            fields,
            lines: (record.start_line, record.end_line),
        });
    }

    fn records(source: &str) -> Vec<Recorded> {
        let mut records = Vec::new();
        unsafe {
            let parser = ts_parser_new();
            assert!(ts_parser_set_language(parser, tree_sitter_hcl()));
            let tree = ts_parser_parse_string(
                parser,
                std::ptr::null(),
                source.as_ptr().cast(),
                source.len().try_into().unwrap(),
            );
            assert!(!tree.is_null());
            cbm_rust_tf_k8s_records(
                ts_tree_root_node(tree),
                source.as_ptr(),
                source.len(),
                (&mut records as *mut Vec<Recorded>).cast(),
                collect,
            );
            ts_tree_delete(tree);
            ts_parser_delete(parser);
        }
        records
    }

    #[test]
    fn terraform_kubernetes_records_keep_block_order_and_locations() {
        let source = r#"# release and resources from the same module
resource "helm_release" "web" {
    chart = "./charts/web"
}
resource "kubernetes_service_v1" "web" {
    metadata {
        name = "web"
        namespace = "apps"
    }
}
resource "kubernetes_manifest" "certificate" {
    manifest = {
        "apiVersion" = "cert-manager.io/v1"
        kind = "Certificate"
        metadata = {
            name = "tls"
            namespace = var.namespace
        }
    }
}
resource "kubernetes_default_service_account" "apps" {
    metadata {
        namespace = "apps"
    }
}
"#;
        let expected = [
            (
                [
                    "resource.kubernetes_service_v1.web",
                    "core",
                    "Service",
                    "",
                    "web",
                    "apps",
                ],
                (5, 10),
            ),
            (
                [
                    "resource.kubernetes_manifest.certificate",
                    "cert-manager.io",
                    "Certificate",
                    "v1",
                    "tls",
                    "",
                ],
                (11, 20),
            ),
            (
                [
                    "resource.kubernetes_default_service_account.apps",
                    "core",
                    "ServiceAccount",
                    "",
                    "default",
                    "apps",
                ],
                (21, 25),
            ),
        ]
        .map(|(fields, lines)| Recorded {
            fields: fields.map(str::to_owned),
            lines,
        });
        assert_eq!(records(source), expected);
    }

    #[test]
    fn terraform_kubernetes_records_skip_computed_and_invalid_names() {
        let source = r#"
resource "kubernetes_service" "computed" {
    metadata { name = var.name }
}
resource "kubernetes_service" "interpolated" {
    metadata { name = "${path.module}" }
}
resource "kubernetes_service" "slash" {
    metadata { name = "wrong/key" }
}
resource "kubernetes_annotations" "patch" {
    metadata { name = "existing" }
}
data "kubernetes_service" "lookup" {
    metadata { name = "existing" }
}
"#;
        assert!(records(source).is_empty());
    }
}
