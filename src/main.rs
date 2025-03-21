use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zbus::connection::Builder as ConnectionBuilder;
use zbus::proxy;
use zbus::zvariant::OwnedObjectPath;
use zbus::zvariant::Type;
use zbus::Connection;

#[proxy(
    interface = "org.a11y.Bus",
    default_path = "/org/a11y/bus",
    default_service = "org.a11y.Bus"
)]
trait A11yBus {
    async fn get_address(&self) -> zbus::Result<String>;
}

#[proxy(
    interface = "org.a11y.atspi.Accessible",
    default_path = "/org/a11y/atspi/accessible/root",
    default_service = "org.a11y.atspi.Registry"
)]
trait Accessible {
    async fn get_children(&self) -> zbus::Result<Vec<ObjectInfo>>;
    async fn get_interfaces(&self) -> zbus::Result<Vec<String>>;
}

#[proxy(interface = "org.freedesktop.DBus.Properties")]
trait Properties {
    async fn get<'a>(
        &'a self,
        interface: &str,
        property: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedValue>;
    async fn get_all(
        &self,
        interface_name: &str,
    ) -> zbus::Result<HashMap<String, zbus::zvariant::OwnedValue>>;
}

#[proxy(interface = "org.a11y.atspi.Text")]
trait Text {
    async fn get_text(&self, start_index: i32, end_index: i32) -> zbus::Result<String>;
}
#[proxy(interface = "org.a11y.atspi.Image")]
trait Image {
    async fn image_description(&self, path: &str) -> zbus::Result<String>;
}

#[derive(Debug, Serialize, Deserialize, Type)]
struct ObjectInfo {
    name: String,
    path: OwnedObjectPath,
}

#[derive(Debug, Serialize, Deserialize)]
struct Properties {
    name: Option<String>,
    description: Option<String>,
    text: Option<String>,
    image_description: Option<String>,
    role_name: Option<String>,
    interfaces: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Node {
    id: u32,
    object_info: ObjectInfo,
    children: Option<Vec<u32>>,
    properties: Properties,
}

async fn get_properties_and_child_nodes(
    connection: &Connection,
    base: Option<ObjectInfo>,
    element_id: u32,
) -> (u32, Option<Properties>, Vec<ObjectInfo>, Option<ObjectInfo>) {
    if let Some(base) = base {
        let properties = PropertiesProxy::builder(&connection)
            .destination(base.name.clone())
            .expect("Failed to create proxy builder")
            .path(base.path.clone())
            .expect("Failed to set path")
            .build()
            .await
            .expect("Failed to create proxy");
        let proxy = AccessibleProxy::builder(&connection)
            .destination(base.name.clone())
            .expect("Failed to create proxy builder")
            .path(base.path.clone())
            .expect("Failed to set path")
            .build()
            .await
            .expect("Failed to create proxy");
        let properties_map = properties
            .get_all("org.a11y.atspi.Accessible")
            .await
            .expect("Failed to get properties");
        let children = proxy.get_children().await;
        let interfaces = proxy
            .get_interfaces()
            .await
            .expect("Failed to get interfaces");
        let mut text: Option<String> = None;
        let mut image_description: Option<String> = None;
        if interfaces.contains(&"org.a11y.atspi.Text".to_string()) {
            let text_proxy = TextProxy::builder(&connection)
                .destination(base.name.clone())
                .expect("Failed to create proxy builder")
                .path(base.path.clone())
                .expect("Failed to set path")
                .build()
                .await
                .expect("Failed to create proxy");
            text = Some(
                text_proxy
                    .get_text(0, -1)
                    .await
                    .expect("Failed to get text"),
            );
        }
        if interfaces.contains(&"org.a11y.atspi.Image".to_string()) {
            image_description = Some(
                properties
                    .get("org.a11y.atspi.Image", "ImageDescription")
                    .await
                    .expect("Failed to get image description")
                    .to_string(),
            );
        }
        let node_properties = Properties {
            name: properties_map.get("Name").map(|v| v.to_string()),
            description: properties_map.get("Description").map(|v| v.to_string()),
            text,
            image_description,
            role_name: properties_map.get("RoleName").map(|v| v.to_string()),
            interfaces,
        };
        if base.path.ends_with("null") {
            (element_id, Some(node_properties), vec![], Some(base))
        } else {
            (
                element_id,
                Some(node_properties),
                children.expect("Failed to get children"),
                Some(base),
            )
        }
    } else {
        let proxy = AccessibleProxy::new(&connection)
            .await
            .expect("Failed to create proxy");
        (
            element_id,
            None,
            proxy.get_children().await.expect("Failed to get children"),
            base,
        )
    }
}

async fn dump_desktop() -> Vec<Node> {
    let connection = ConnectionBuilder::session()
        .expect("Failed to create connection builder")
        .build()
        .await
        .expect("Failed to create connection");
    let bus = A11yBusProxy::new(&connection)
        .await
        .expect("Failed to create proxy");
    let address = bus.get_address().await.expect("Failed to get address");
    let connection = ConnectionBuilder::address(address.as_str())
        .expect("Failed to create connection builder")
        .build()
        .await
        .expect("Failed to create connection");
    let initial_properties_and_children =
        get_properties_and_child_nodes(&connection, None, 0).await;
    let mut latest_idx = 1;
    let mut futures = FuturesUnordered::new();
    for child in initial_properties_and_children.2 {
        let connection = &connection;
        futures.push(get_properties_and_child_nodes(
            connection,
            Some(child),
            latest_idx,
        ));
        latest_idx += 1;
    }
    let mut all_nodes = vec![];
    while let Some((element_id, properties, children, object_info)) = futures.next().await {
        let mut children_ids: Vec<u32> = vec![];
        for c in children {
            futures.push(get_properties_and_child_nodes(
                &connection,
                Some(c),
                latest_idx,
            ));
            children_ids.push(latest_idx);
            latest_idx += 1;
        }
        if let Some(properties) = properties {
            let node = Node {
                id: element_id,
                properties,
                children: Some(children_ids),
                object_info: object_info.expect("Must have object info if there are properties"),
            };
            all_nodes.push(node);
        }
    }
    all_nodes
}

async fn tree() {
    let nodes = dump_desktop().await;
    
    for node in nodes.iter() {
        println!("{:?}", node);
    }
}

#[tokio::main]
async fn main() {
    tree().await;
}
