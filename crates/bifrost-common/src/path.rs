use crate::BifrostError;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathIssue {
    Absolute,
    ParentTraversal,
    EmptyComponent,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemotePath(String);

impl RemotePath {
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn parse(value: &str) -> Result<Self, BifrostError> {
        let normalized = value.replace('\\', "/");
        if normalized.starts_with('/') || normalized.contains(':') {
            return Err(BifrostError::InvalidPath(format!("absolute path: {value}")));
        }

        let mut components = Vec::new();
        for component in normalized.split('/') {
            match component {
                "" | "." => continue,
                ".." => {
                    return Err(BifrostError::InvalidPath(format!(
                        "parent traversal: {value}"
                    )));
                }
                component => components.push(component),
            }
        }
        Ok(Self(components.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, name: &str) -> Result<Self, BifrostError> {
        let candidate = if self.0.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", self.0, name)
        };
        Self::parse(&candidate)
    }
}

impl fmt::Debug for RemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("RemotePath").field(&self.0).finish()
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::RemotePath;

    #[test]
    fn normalizes_separators_and_dot_components() {
        assert_eq!(
            RemotePath::parse(r"docs\\./report.txt").unwrap().as_str(),
            "docs/report.txt"
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(RemotePath::parse("docs/../secret.txt").is_err());
    }
}
