use log::{debug, info, warn};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    process::Command,
};
use tokio::task::JoinSet;
pub struct ServiceHandler {
    service_map: HashMap<String, String>,
}
const NO_ASSOCIATED_SERVICE: &str = "NONE";
impl ServiceHandler {
    pub fn new() -> Self {
        let lookup: HashMap<String, Value> =
            serde_json::from_str(include_str!("services.json")).unwrap();
        let mut map = HashMap::new();
        for (key, val) in lookup.iter() {
            if let Some(v) = val.as_str() {
                map.insert(key.to_owned(), v.to_owned());
            }
        }
        ServiceHandler { service_map: map }
    }

    pub fn stop_services<'a, I>(&self, topics: I) -> JoinSet<()>
    where
        I: IntoIterator<Item = &'a String>,
    {
        let mut services = HashSet::new();
        for t in topics {
            let service_name = self.topic_to_service(t);
            debug!("topic {} refers to service {}", t, service_name);
            if service_name == NO_ASSOCIATED_SERVICE {
                continue;
            }
            services.insert(service_name);
        }
        let mut tasks = JoinSet::new();
        for service_name in services {
            tasks.spawn(Self::stop_service(service_name));
        }
        tasks
    }

    fn topic_to_service(&self, topic: &str) -> String {
        let topic = if let Some(t) = topic.strip_prefix("rt/") {
            t
        } else if let Some(t) = topic.strip_prefix("/") {
            t
        } else {
            topic
        };

        let topic = topic.split("/").next().unwrap();
        if self.service_map.contains_key(topic) {
            self.service_map[topic].to_owned()
        } else {
            NO_ASSOCIATED_SERVICE.to_owned()
        }
    }

    async fn stop_service(service_name: String) {
        debug!("Stopping service {}", service_name);
        let out = Command::new("systemctl")
            .arg("stop")
            .arg(&service_name)
            .output();
        match out {
            Err(e) => warn!("Error when stopping service {}: {:?}", service_name, e),
            Ok(v) if !v.stderr.is_empty() => {
                warn!("Output when stopping service {}: {:?}", service_name, v)
            }
            Ok(v) => debug!("Output when stopping service {}: {:?}", service_name, v),
        }
        info!("Stopped service {}", service_name);
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceHandler;

    #[test]
    fn test_service_to_topic() {
        let s = ServiceHandler::new();
        let service = s.topic_to_service("/camera/h264");
        assert_eq!(
            service, "camera",
            "Topic was /camera/h264 and got service {}",
            service
        )
    }
}
