use std::convert::TryFrom;
use lazy_static::lazy_static;
use futures::try_join;
use snafu::{Backtrace, Snafu, ResultExt, futures::try_future::{TryFutureExt}};
use kube::{api::{ListParams, PatchParams, PatchStrategy}};
use k8s_openapi::api::core::v1::{Pod, Node, Volume, Taint};
use std::collections::HashMap;
use tracing::{instrument, event, Level};

// https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/using-eni.html#AvailableIpPerENI shows a
// table of how many ENIs are available for node types... in general this maps to the following:
lazy_static! {
    static ref ENI_COUNT_BY_INSTANCE_SIZE: HashMap<&'static str, u8> = vec![
        ("nano", 2u8),
        ("micro", 2u8),
        ("small", 3u8),
        ("medium", 2u8),
        ("large", 3u8),
        ("xlarge", 4u8),
        ("2xlarge", 4u8),
        ("4xlarge", 8u8),
        ("8xlarge", 8u8),
        ("9xlarge", 8u8),
        ("10xlarge", 8u8),
        ("12xlarge", 8u8),
        ("16xlarge", 15u8),
        ("18xlarge", 15u8),
        ("24xlarge", 15u8),
    ].into_iter().collect();
}

// For .metal and anything we don't recognize, we assume the worst that it can attach 15 ENIs... different classes do different things.
const DEFAULT_ENI_LIMIT: u8 = 15;

const LIMIT_ANNOTATION: &str = "xometry.com/ebs-limit";
const TAINT_KEY: &str = "xometry.com/ebs-limit-reached";
const INSTANCE_TYPE_LABEL: &str = "beta.kubernetes.io/instance-type";

#[derive(Debug, Snafu)]
pub enum Error {
    KubeFailure {
        source: kube::error::Error,
        backtrace: Backtrace
    },

    SerializationError {
        source: serde_json::error::Error,
        backtrace: Backtrace
    },
}

fn get_ebs_limit_from_annotation(node: &Node) -> Option<u8> {
    node.metadata.annotations.as_ref()?.get(LIMIT_ANNOTATION)?.parse().ok()
}

// Nitro instances are documented to allow 28 attachments, no matter the instance size. In
// practice we sometimes see a few more than that, up to 32! But we stick do the documented
// size, reserve one for the root EBS volume, one for the local SSD volume, and reserve space
// for the documented number of ENIs.
fn get_ebs_limit_from_instance_type(node: &Node) -> u8 {
    let instance_type: &str = node.metadata.labels.as_ref().and_then(|labels| labels.get(INSTANCE_TYPE_LABEL)).map_or("", |s| &s);
    let instance_size = instance_type.split(".").last().unwrap();
    let eni_limit = ENI_COUNT_BY_INSTANCE_SIZE.get(instance_size).map_or(DEFAULT_ENI_LIMIT, |v| *v);
    28 - 2 - eni_limit
}

fn get_ebs_limit_for_node(node: &Node) -> u8 {
    get_ebs_limit_from_annotation(node).unwrap_or_else(|| get_ebs_limit_from_instance_type(node))
}

fn volume_is_ebs(volume: &Volume) -> bool {
    // For the purposes of this controller, we assume that all PVCs are backed by EBS. This is... not true! But good enough for us.
    volume.persistent_volume_claim.is_some()
}

fn ebs_volume_count(pod: &Pod) -> u8 {
    match &pod.spec.as_ref().unwrap().volumes {
        None => 0,
        Some(volumes) => {
            let i = volumes.iter();
            u8::try_from(i.filter(|volume| volume_is_ebs(volume)).count()).unwrap_or(255)
        }
    }
}

fn pod_node_name(pod: &Pod) -> Option<&String> {
    pod.spec.as_ref()?.node_name.as_ref()
}

fn node_has_taint(node: &Node) -> bool {
    node.spec.as_ref().unwrap().taints.as_ref().map_or(false, |taints| {
        taints.iter().any(|taint| taint.key == TAINT_KEY)
    })
}

#[instrument(skip(client))]
async fn get_node_volume_counts(client: kube::Client) -> Result<HashMap<String, u8>, Error> {
    let api: kube::Api<Pod> = kube::Api::all(client);
    let pods = api.list(&ListParams::default()).context(KubeFailure {}).await?;

    let mut node_map: HashMap<String, u8> = HashMap::new();
    for pod in pods.items.iter() {
        match pod_node_name(pod) {
            None => (),
            Some(node_name) => {
                let entry = node_map.entry(node_name.clone()).or_insert(0);
                *entry += ebs_volume_count(pod);
            }
        };
    }
    Ok(node_map)
}

#[instrument(skip(client))]
async fn get_nodes(client: kube::Client) -> Result<Vec<Node>, Error> {
    let api: kube::Api<Node> = kube::Api::all(client);
    let nodes = api.list(&ListParams::default()).context(KubeFailure {}).await?;
    Ok(nodes.items)
}

#[instrument(skip(client, node), fields(node_name = %node.metadata.name.as_ref().unwrap()))]
async fn taint_node(client: kube::Client, node: &Node) -> Result<(), Error> {
    let mut taints = node.spec.as_ref().unwrap().taints.clone().unwrap_or_else(|| Vec::new());
    taints.push(Taint {
        effect: String::from("NoSchedule"),
        key: String::from(TAINT_KEY),
        value: Some(String::from("true")),
        time_added: None,
    });
    let api: kube::Api<Node> = kube::Api::all(client);
    let data = serde_json::to_vec(&serde_json::json!({
        "spec": {
            "taints": taints
        }
    })).context(SerializationError {})?;
    let params = PatchParams {
        dry_run: false,
        patch_strategy: PatchStrategy::Strategic,
        force: false,
        field_manager: None,
    };
    api.patch(node.metadata.name.as_ref().unwrap(), &params, data).context(KubeFailure {}).await?;
    Ok(())
}

#[instrument(skip(client, node), fields(node_name = %node.metadata.name.as_ref().unwrap()))]
async fn untaint_node(client: kube::Client, node: &Node) -> Result<(), Error> {
    let taint_vec: Vec<&Taint> = node.spec.as_ref().unwrap().taints.as_ref().unwrap().iter().filter(|taint| taint.key != TAINT_KEY).collect();
    let taints = if taint_vec.is_empty() { None } else { Some(taint_vec) };
    let api: kube::Api<Node> = kube::Api::all(client);
    let data = serde_json::to_vec(&serde_json::json!({
        "spec": {
            "taints": taints
        }
    })).context(SerializationError {})?;
    let params = PatchParams {
        dry_run: false,
        patch_strategy: PatchStrategy::Strategic,
        force: false,
        field_manager: None,
    };
    api.patch(node.metadata.name.as_ref().unwrap(), &params, data).context(KubeFailure {}).await?;
    Ok(())
}

#[instrument(skip(client, node, volume_count), fields(node_name = %node.metadata.name.as_ref().unwrap()))]
async fn reconcile_node(client: kube::Client, node: &Node, volume_count: u8) -> Result<(), Error> {
    let node_name = node.metadata.name.as_ref().unwrap();
    let ebs_limit = get_ebs_limit_for_node(node);
    let is_tainted = node_has_taint(node);
    let should_be_tainted = volume_count >= ebs_limit;
    if should_be_tainted {
        if !is_tainted {
            event!(Level::INFO, %node_name, volume_count, ebs_limit, "tainting node");
            taint_node(client.clone(), node).await?;
        } else {
            event!(Level::INFO, %node_name, volume_count, ebs_limit, "node remains tainted");
        }
    } else {
        if is_tainted {
            event!(Level::INFO, %node_name, volume_count, ebs_limit, "untainting taint");
            untaint_node(client.clone(), node).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .init();

    let client = kube::Client::try_default().context(KubeFailure {}).await?;

    let (nodes, node_map) = try_join!(
        get_nodes(client.clone()),
        get_node_volume_counts(client.clone()),
    )?;

    let results = futures::future::join_all(nodes.iter().map(|node| {
        let node_name = node.metadata.name.as_ref().unwrap();
        let volume_count = node_map.get(node_name).copied().unwrap_or(0);
        reconcile_node(client.clone(), node, volume_count)
    })).await;
    for result in results {
        match result {
            Err(e) => { return Err(e); }
            Ok(_) => (),
        }
    }
    Ok(())
}
