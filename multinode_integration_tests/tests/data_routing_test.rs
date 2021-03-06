// Copyright (c) 2017-2018, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
extern crate multinode_integration_tests_lib;
extern crate node_lib;
extern crate regex;
extern crate serde_cbor;
extern crate sub_lib;
extern crate hopper_lib;
extern crate base64;
extern crate neighborhood_lib;
extern crate test_utils;

use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;
use neighborhood_lib::gossip::Gossip;
use neighborhood_lib::gossip::GossipNodeRecord;
use neighborhood_lib::gossip::NeighborRelationship;
use node_lib::json_masquerader::JsonMasquerader;
use sub_lib::cryptde::Key;
use sub_lib::dispatcher::Component;
use sub_lib::hopper::IncipientCoresPackage;
use sub_lib::proxy_server::ClientRequestPayload;
use sub_lib::proxy_client::ClientResponsePayload;
use sub_lib::proxy_server::ProxyProtocol;
use sub_lib::route::Route;
use sub_lib::route::RouteSegment;
use sub_lib::utils::index_of;
use sub_lib::utils::plus;
use multinode_integration_tests_lib::substratum_node_cluster::SubstratumNodeCluster;
use multinode_integration_tests_lib::substratum_real_node::NodeStartupConfigBuilder;
use multinode_integration_tests_lib::substratum_node::NodeReference;
use multinode_integration_tests_lib::substratum_node::SubstratumNode;
use multinode_integration_tests_lib::substratum_node::PortSelector;
use sub_lib::cryptde_null::CryptDENull;
use std::collections::HashSet;
use std::thread;
use sub_lib::sequence_buffer::SequencedPacket;
use test_utils::test_utils::make_meaningless_stream_key;

#[test]
fn http_request_to_cores_package_and_cores_package_to_http_response_test () {
    let masquerader = JsonMasquerader::new ();
    let mut cluster = SubstratumNodeCluster::start ().unwrap ();
    let mock_bootstrap = cluster.start_mock_node(vec!(5550));
    let mock_standard = cluster.start_mock_node(vec!(5551));
    let subject = cluster.start_real_node(NodeStartupConfigBuilder::standard()
        .bootstrap_from(mock_bootstrap.node_reference())
        .build());

    let (_, _, package) = mock_bootstrap.wait_for_package (&masquerader, Duration::from_millis (1000)).unwrap ();
    let incoming_cores_package = package.to_expired (mock_bootstrap.cryptde ());
    let incoming_gossip = incoming_cores_package.payload::<Gossip> ().unwrap ();
    assert_eq! (incoming_gossip.node_records.into_iter ().collect::<HashSet<GossipNodeRecord>> (), vec! (
        GossipNodeRecord {
            public_key: mock_bootstrap.public_key (),
            node_addr_opt: None,
            is_bootstrap_node: true,
        },
        GossipNodeRecord {
            public_key: subject.public_key (),
            node_addr_opt: Some (subject.node_addr ()),
            is_bootstrap_node: false,
        },
    ).into_iter ().collect::<HashSet<GossipNodeRecord>> ());

    let ne1_noderef = NodeReference::new (Key::new (&b"ne1"[..]), IpAddr::from_str ("100.100.100.001").unwrap (), vec! (5561));
    let ne2_noderef = NodeReference::new (Key::new (&b"ne2"[..]), IpAddr::from_str ("100.100.100.002").unwrap (), vec! (5562));
    let ne3_noderef = NodeReference::new (Key::new (&b"ne3"[..]), IpAddr::from_str ("100.100.100.003").unwrap (), vec! (5563));
    let outgoing_gossip = make_gossip (vec! ((&subject.node_reference (), false), (&mock_standard.node_reference (), true), (&ne1_noderef, false), (&ne2_noderef, false), (&ne3_noderef, false)));
    let route = Route::new (vec! (
        RouteSegment::new (vec! (&mock_bootstrap.public_key (), &subject.public_key ()), Component::Neighborhood)
    ), mock_bootstrap.cryptde ()).unwrap ();
    let outgoing_package = IncipientCoresPackage::new (route, outgoing_gossip, &subject.public_key ());
    mock_bootstrap.transmit_package (5551, outgoing_package, &masquerader, &subject.public_key (), subject.socket_addr (PortSelector::First)).unwrap ();

    // It'd be nice if there were a better way to ensure that the Gossip had been consumed.
    thread::sleep (Duration::from_millis (1000));

    let mut client = subject.make_client (80);
    client.send_chunk (Vec::from (&b"GET / HTTP/1.1\r\nHost: www.example.com\r\n\r\n"[..]));

    let (_, _, package) = mock_standard.wait_for_package(&masquerader, Duration::from_millis (1000)).unwrap ();
    let incoming_cores_package = package.to_expired (&CryptDENull::from (&ne2_noderef.public_key));
    let request_payload = incoming_cores_package.payload::<ClientRequestPayload> ().unwrap ();
    assert_eq! (request_payload.last_data, false);
    assert_eq! (request_payload.sequenced_packet.sequence_number, 0);
    assert_eq! (request_payload.sequenced_packet.data, Vec::from (&b"GET / HTTP/1.1\r\nHost: www.example.com\r\n\r\n"[..]));
    assert_eq! (request_payload.target_hostname, Some (String::from ("www.example.com")));
    assert_eq! (request_payload.target_port, 80);
    assert_eq! (request_payload.protocol, ProxyProtocol::HTTP);
    assert_eq! (request_payload.originator_public_key, subject.public_key ());

    let route = Route::new (vec! (
        RouteSegment::new (vec! (&mock_standard.public_key (), &subject.public_key ()), Component::ProxyServer)
    ), mock_standard.cryptde ()).unwrap ();
    let response_payload = ClientResponsePayload {
        stream_key: request_payload.stream_key,
        last_response: false,
        sequenced_packet: SequencedPacket {data: b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 21\r\n\r\nOoh! Do you work out?".to_vec (), sequence_number: 0},
    };
    let outgoing_package = IncipientCoresPackage::new (route, response_payload, &subject.public_key ());
    mock_standard.transmit_package (5551, outgoing_package, &masquerader, &subject.public_key (), subject.socket_addr (PortSelector::First)).unwrap ();

    let response = client.wait_for_chunk ();
    assert_eq! (index_of (&response, &b"Ooh! Do you work out?"[..]).is_some (), true);
}

#[test]
fn cores_package_to_http_request_and_http_response_to_cores_package_test () {
    let masquerader = JsonMasquerader::new ();
    let mut cluster = SubstratumNodeCluster::start ().unwrap ();
    let mock_bootstrap = cluster.start_mock_node(vec!(5550));
    let mock_standard = cluster.start_mock_node(vec!(5551));
    let subject = cluster.start_real_node(NodeStartupConfigBuilder::standard()
        .bootstrap_from(mock_bootstrap.node_reference())
        .build());

    mock_bootstrap.wait_for_package (&masquerader, Duration::from_millis (1000)).unwrap ();

    let ne1_noderef = NodeReference::new (Key::new (&b"ne1"[..]), IpAddr::from_str ("100.100.100.001").unwrap (), vec! (5561));
    let ne2_noderef = NodeReference::new (Key::new (&b"ne2"[..]), IpAddr::from_str ("100.100.100.002").unwrap (), vec! (5562));
    let ne3_noderef = NodeReference::new (Key::new (&b"ne3"[..]), IpAddr::from_str ("100.100.100.003").unwrap (), vec! (5563));
    let outgoing_gossip = make_gossip (vec! ((&ne1_noderef, false), (&ne2_noderef, false), (&ne3_noderef, false), (&mock_standard.node_reference (), true)));
    let route = Route::new (vec! (
        RouteSegment::new (vec! (&mock_bootstrap.public_key (), &subject.public_key ()), Component::Neighborhood)
    ), mock_bootstrap.cryptde ()).unwrap ();
    let outgoing_package = IncipientCoresPackage::new (route, outgoing_gossip, &subject.public_key ());
    mock_bootstrap.transmit_package (5551, outgoing_package, &masquerader, &subject.public_key (), subject.socket_addr (PortSelector::First)).unwrap ();

    let client_request_payload = ClientRequestPayload {
        stream_key: make_meaningless_stream_key (),
        last_data: true,
        sequenced_packet: SequencedPacket {data: b"GET / HTTP/1.1\r\nHost: www.example.com\r\n\r\n".to_vec (), sequence_number: 0},
        target_hostname: Some (String::from ("www.example.com")),
        target_port: 80,
        protocol: ProxyProtocol::HTTP,
        originator_public_key: ne1_noderef.public_key.clone (),
    };
    let route = Route::new (vec! (
        RouteSegment::new (vec! (&mock_standard.public_key (), &subject.public_key ()), Component::ProxyClient),
        RouteSegment::new (vec! (&subject.public_key (), &mock_standard.public_key (), &ne3_noderef.public_key, &ne2_noderef.public_key, &ne1_noderef.public_key), Component::ProxyServer),
    ), mock_standard.cryptde ()).unwrap ();
    let outgoing_package = IncipientCoresPackage::new (route, client_request_payload, &subject.public_key ());
    mock_standard.transmit_package (5551, outgoing_package, &masquerader, &subject.public_key (), subject.socket_addr (PortSelector::First)).unwrap ();

    let (_, _, package) = mock_standard.wait_for_package (&masquerader, Duration::from_millis (1000)).unwrap ();
    let incoming_cores_package = package.to_expired (&CryptDENull::from (&ne1_noderef.public_key));
    let client_response_payload = incoming_cores_package.payload::<ClientResponsePayload> ().unwrap ();
    assert_eq! (client_response_payload.stream_key, make_meaningless_stream_key ());
    assert_eq! (client_response_payload.last_response, false);
    assert_eq! (client_response_payload.sequenced_packet.sequence_number, 0);
    assert_eq! (index_of (&client_response_payload.sequenced_packet.data,
        &b"This domain is established to be used for illustrative examples in documents."[..]).is_some (), true);
}

#[test]
fn end_to_end_gossip_and_routing_test () {
    let mut cluster = SubstratumNodeCluster::start ().unwrap ();
    let bootstrap_node = cluster.start_real_node (NodeStartupConfigBuilder::bootstrap ().build ());
    let originating_node = cluster.start_real_node (NodeStartupConfigBuilder::standard ().bootstrap_from (bootstrap_node.node_reference ()).build ());
    thread::sleep (Duration::from_millis (1000));

    cluster.start_real_node (NodeStartupConfigBuilder::standard ().bootstrap_from (bootstrap_node.node_reference ()).build ());
    cluster.start_real_node (NodeStartupConfigBuilder::standard ().bootstrap_from (bootstrap_node.node_reference ()).build ());
    cluster.start_real_node (NodeStartupConfigBuilder::standard ().bootstrap_from (bootstrap_node.node_reference ()).build ());
    let mut client = originating_node.make_client (80);

    client.send_chunk (Vec::from (&b"GET / HTTP/1.1\r\nHost: www.example.com\r\n\r\n"[..]));
    let response = client.wait_for_chunk();

    assert_eq! (index_of (&response,
        &b"This domain is established to be used for illustrative examples in documents."[..]).is_some (), true);
}

#[test]
fn multiple_stream_zero_hop_test () {
    let mut cluster = SubstratumNodeCluster::start ().unwrap ();
    let zero_hop_node = cluster.start_real_node (NodeStartupConfigBuilder::zero_hop().build ());
    let mut one_client = zero_hop_node.make_client (80);
    let mut another_client = zero_hop_node.make_client (80);

    one_client.send_chunk (Vec::from (&b"GET / HTTP/1.1\r\nHost: www.example.com\r\n\r\n"[..]));
    another_client.send_chunk (Vec::from (&b"GET / HTTP/1.1\r\nHost: www.fallingfalling.com\r\n\r\n"[..]));

    let one_response = one_client.wait_for_chunk();
    let another_response = another_client.wait_for_chunk ();

    assert_eq! (index_of (&one_response,
        &b"This domain is established to be used for illustrative examples in documents."[..]).is_some (), true);
    assert_eq! (index_of (&another_response,
        &b"FALLING FALLING .COM BY RAFAEL ROZENDAAL"[..]).is_some (), true);
}

fn make_gossip (pairs: Vec<(&NodeReference, bool)>) -> Gossip {
    let node_ref_count = pairs.len () as u32;
    let gossip_node_records = pairs.into_iter ()
        .fold (vec! (), |so_far, pair| {
            let (node_ref_ref, reveal) = pair;
            plus (so_far, GossipNodeRecord {
                public_key: node_ref_ref.public_key.clone (),
                node_addr_opt: if reveal {Some (node_ref_ref.node_addr.clone ())} else {None},
                is_bootstrap_node: false,
            })
        });
    let mut gossip_neighbor_pairs = vec! ();
    for i in 0..(node_ref_count - 1) {
        gossip_neighbor_pairs.push (NeighborRelationship {from: i, to: (i + 1)})
    }
    for i in 1..node_ref_count {
        gossip_neighbor_pairs.push (NeighborRelationship {from: i, to: (i - 1)})
    }
    Gossip {
        node_records: gossip_node_records,
        neighbor_pairs: gossip_neighbor_pairs,
    }
}