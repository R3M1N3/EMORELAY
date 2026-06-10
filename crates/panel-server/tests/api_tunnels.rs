mod common;

use panel_server::models::tunnel::{Tunnel, TunnelHop};

#[tokio::test]
async fn create_tunnel_with_hops_and_read_back() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('hk', 'x', '1.1.1.1')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('jp', 'x', '2.2.2.2')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();

    let tid = Tunnel::create_with_hops(
        &app.state.pool, "hk-jp", "tcp",
        &[(0, n1, None), (1, n2, Some(30001))],
    ).await.unwrap();

    let t = Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().unwrap();
    assert_eq!(t.name, "hk-jp");
    assert_eq!(t.transport, "tcp");
    assert_eq!(t.status, "unknown");

    let hops = TunnelHop::list_for_tunnel(&app.state.pool, tid).await.unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].ordinal, 0);
    assert_eq!(hops[0].node_id, n1);
    assert!(hops[0].inter_port.is_none());
    assert_eq!(hops[1].ordinal, 1);
    assert_eq!(hops[1].inter_port, Some(30001));
}

#[tokio::test]
async fn soft_delete_hides_tunnel_and_active_refs_counts() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let tid = Tunnel::create_with_hops(&app.state.pool, "t1", "tls",
        &[(0, n1, None), (1, n2, Some(30002))]).await.unwrap();

    assert_eq!(Tunnel::active_rule_refs(&app.state.pool, tid).await.unwrap(), 0);
    assert_eq!(Tunnel::soft_delete(&app.state.pool, tid).await.unwrap(), 1);
    assert!(Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().is_none());
}

#[tokio::test]
async fn hops_using_node_detects_node_membership() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    Tunnel::create_with_hops(&app.state.pool, "t2", "tcp",
        &[(0, n1, None), (1, n2, Some(30003))]).await.unwrap();
    assert!(TunnelHop::node_in_active_tunnel(&app.state.pool, n2).await.unwrap());
    let n3 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('c','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    assert!(!TunnelHop::node_in_active_tunnel(&app.state.pool, n3).await.unwrap());
}
