use std::fmt;

pub const ENVS: [&str; 3] = ["dev", "staging", "prod"];

/// Data-plane permission level (what a key or a user may do inside one project environment).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Role {
    Read = 1,
    Write = 2,
    Admin = 3,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "read" => Some(Role::Read),
            "write" => Some(Role::Write),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Read => "read",
            Role::Write => "write",
            Role::Admin => "admin",
        }
    }
}

/// Membership level inside an organization.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum OrgRole {
    Member = 1,
    Admin = 2,
    Owner = 3,
}

impl OrgRole {
    pub fn parse(s: &str) -> Option<OrgRole> {
        match s {
            "member" => Some(OrgRole::Member),
            "admin" => Some(OrgRole::Admin),
            "owner" => Some(OrgRole::Owner),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            OrgRole::Member => "member",
            OrgRole::Admin => "admin",
            OrgRole::Owner => "owner",
        }
    }
    /// What organization members may do on project data when they use their own login.
    pub fn data_role(&self) -> Role {
        match self {
            OrgRole::Member => Role::Read,
            OrgRole::Admin | OrgRole::Owner => Role::Admin,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Principal {
    User { user_id: String },
    Key { key_id: String, project_id: String, env: String, role: Role },
}

impl Principal {
    /// Identifier used for rate limiting and logs.
    pub fn id(&self) -> &str {
        match self {
            Principal::User { user_id } => user_id,
            Principal::Key { key_id, .. } => key_id,
        }
    }
    /// Identifier recorded in the audit log.
    pub fn actor(&self) -> String {
        match self {
            Principal::User { user_id } => format!("user:{}", user_id),
            Principal::Key { key_id, .. } => format!("key:{}", key_id),
        }
    }
}

#[derive(Clone)]
pub struct ClientIp(pub String);

#[derive(Clone)]
pub struct TokenHash(pub String);

#[derive(Clone)]
pub struct KeyId(pub String);

impl fmt::Display for ClientIp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
