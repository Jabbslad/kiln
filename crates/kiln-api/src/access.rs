//! Box visibility for the server edge and host API.
use crate::auth::{Caller, Role};

/// Whether `caller` may use door SSH, the browser terminal, or see a box on the dashboard.
pub fn may_access_box(caller: &Caller, box_owner: Option<&str>, tenant_owner: &str) -> bool {
    match caller.role {
        Role::Owner => true,
        Role::Member => {
            let owner = box_owner
                .filter(|id| !id.is_empty())
                .unwrap_or(tenant_owner);
            caller.sub == owner
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    fn member(sub: &str, tenant: &str) -> Caller {
        Caller {
            sub: sub.into(),
            tenant_id: tenant.into(),
            role: Role::Member,
        }
    }

    fn owner(sub: &str, tenant: &str) -> Caller {
        Caller {
            sub: sub.into(),
            tenant_id: tenant.into(),
            role: Role::Owner,
        }
    }

    #[test]
    fn tenant_owner_sees_every_box_on_the_server() {
        assert!(may_access_box(
            &owner("owner-a", "tenant-a"),
            Some("member-b"),
            "owner-a"
        ));
        assert!(may_access_box(
            &owner("owner-a", "tenant-a"),
            None,
            "owner-a"
        ));
    }

    #[test]
    fn tenant_member_only_sees_own_boxes() {
        assert!(may_access_box(
            &member("member-b", "tenant-a"),
            Some("member-b"),
            "owner-a"
        ));
        assert!(!may_access_box(
            &member("member-b", "tenant-a"),
            Some("owner-a"),
            "owner-a"
        ));
        assert!(!may_access_box(
            &member("member-b", "tenant-a"),
            None,
            "owner-a"
        ));
    }
}
