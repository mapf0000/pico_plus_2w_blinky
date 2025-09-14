use core::net::Ipv4Addr;
use embassy_net as net;

#[embassy_executor::task]
pub async fn server_task(stack: &'static net::Stack<'static>) {
    // Pool: 192.168.4.100 – 192.168.4.200
    let server_ip = Ipv4Addr::new(192, 168, 4, 1);
    let mask = Ipv4Addr::new(255, 255, 255, 0);
    let router = Ipv4Addr::new(192, 168, 4, 1);
    // Use router as DNS just to satisfy clients; not forwarding upstream.
    let dns = Ipv4Addr::new(192, 168, 4, 1);
    let pool_start = Ipv4Addr::new(192, 168, 4, 100);
    let pool_end = Ipv4Addr::new(192, 168, 4, 200);

    // Up to 16 leases, up to 2 DNS servers
    let mut dhcp: leasehund::DhcpServer<16, 2> =
        leasehund::DhcpServer::new_with_dns(server_ip, mask, router, dns, pool_start, pool_end);

    log::info!("dhcp: server started on {} with pool {}-{}", server_ip, pool_start, pool_end);
    dhcp.run(*stack).await;
}
