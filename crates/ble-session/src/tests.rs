use super::*;
use provisioning::Profile;
const KEY: [u8; 32] = [7; 32];
const ID: [u8; 16] = [8; 16];
#[test]
fn default_mtu_records_cancel_partial_upload_and_readback() {
    use ble_protocol::{KIND_RECORD, Receiver, Response, fragment, read_fragment};
    let (mut client, mut server) = pair();
    let mut receiver = Receiver::new();
    let mut ciphertext = [0; MAX_CIPHERTEXT];
    let run = [2; MAX_PLAINTEXT];
    let length = client.seal(2, &run, &mut ciphertext).unwrap();
    let (frame, n) = fragment(KIND_RECORD, 2, &ciphertext[..length], 0).unwrap();
    assert!(receiver.push(&frame[..n]).unwrap().is_none());
    // The Radio actor may be interrupted after any fragment. A newer encrypted
    // record abandons its reassembly without reusing the Run's nonce.
    let length = client.seal(3, &[2, 8, 1], &mut ciphertext).unwrap();
    let mut plaintext = [0; MAX_PLAINTEXT];
    for offset in (0..length).step_by(ble_protocol::PAYLOAD_LEN) {
        let (frame, n) = fragment(KIND_RECORD, 3, &ciphertext[..length], offset).unwrap();
        if let Some(message) = receiver.push(&frame[..n]).unwrap() {
            assert_eq!(
                server.open(message.token, message.payload, &mut plaintext),
                Ok(3)
            );
            assert_eq!(&plaintext[..3], &[2, 8, 1]);
        }
    }
    let length = client.seal(4, &run, &mut ciphertext).unwrap();
    for offset in (0..length).step_by(ble_protocol::PAYLOAD_LEN) {
        let (frame, n) = fragment(KIND_RECORD, 4, &ciphertext[..length], offset).unwrap();
        if let Some(message) = receiver.push(&frame[..n]).unwrap() {
            assert_eq!(
                server.open(message.token, message.payload, &mut plaintext),
                Ok(MAX_PLAINTEXT)
            );
            assert_eq!(plaintext, run);
        }
    }
    let snapshot = [42; ble_protocol::SNAPSHOT_LEN];
    let length = server.seal(1, &snapshot, &mut ciphertext).unwrap();
    let mut response = Response::new();
    response.set(KIND_RECORD, 1, &ciphertext[..length]).unwrap();
    let mut received = Receiver::new();
    for _ in 0..4 {
        let frame = response.next_fragment().unwrap();
        if let Some(message) = received.push(read_fragment(&frame).unwrap()).unwrap() {
            assert_eq!(
                client.open(message.token, message.payload, &mut plaintext),
                Ok(snapshot.len())
            );
            assert_eq!(&plaintext[..snapshot.len()], snapshot);
        }
    }
}
#[test]
fn replayed_first_handshake_cannot_reuse_an_authorized_transport() {
    let profile = Profile::new(ID, KEY).unwrap();
    let mut client = Handshake::new(true, &profile, [11; 32]);
    let mut first = [0; HANDSHAKE_LEN];
    client.write(&mut first).unwrap();
    let mut original = Handshake::new(false, &profile, [12; 32]);
    original.read(&first).unwrap();
    let mut answer = [0; HANDSHAKE_LEN];
    original.write(&mut answer).unwrap();
    client.read(&answer).unwrap();
    let mut client = client.finish().unwrap();
    let mut command = [0; 18];
    client.seal(2, &[1, 0], &mut command).unwrap();
    let mut replay = Handshake::new(false, &profile, [13; 32]);
    // NNpsk0's first message is replayable. No application payload is accepted.
    replay.read(&first).unwrap();
    replay.write(&mut answer).unwrap();
    let mut replay = replay.finish().unwrap();
    assert_eq!(
        replay.open(2, &command, &mut [0; 2]),
        Err(Error::Authentication)
    );
}
fn pair() -> (Session, Session) {
    let profile = Profile::new(ID, KEY).unwrap();
    let mut a = Handshake::new(true, &profile, [11; 32]);
    let mut b = Handshake::new(false, &profile, [12; 32]);
    let mut message = [0; HANDSHAKE_LEN];
    a.write(&mut message).unwrap();
    b.read(&message).unwrap();
    b.write(&mut message).unwrap();
    a.read(&message).unwrap();
    (a.finish().unwrap(), b.finish().unwrap())
}
#[test]
fn interoperates_with_snow_in_both_roles_and_directions() {
    for initiator in [true, false] {
        let profile = Profile::new(ID, KEY).unwrap();
        let context = prologue(&ID);
        let mut ours = Handshake::new(initiator, &profile, [11; 32]);
        let builder = snow::Builder::new(SUITE.parse().unwrap())
            .psk(0, &KEY)
            .unwrap()
            .prologue(&context)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&[12; 32]);
        let mut other = if initiator {
            builder.build_responder()
        } else {
            builder.build_initiator()
        }
        .unwrap();
        let mut message = [0; HANDSHAKE_LEN];
        if initiator {
            ours.write(&mut message).unwrap();
            assert_eq!(other.read_message(&message, &mut []).unwrap(), 0);
            assert_eq!(
                other.write_message(&[], &mut message).unwrap(),
                HANDSHAKE_LEN
            );
            ours.read(&message).unwrap();
        } else {
            other.write_message(&[], &mut message).unwrap();
            ours.read(&message).unwrap();
            ours.write(&mut message).unwrap();
            other.read_message(&message, &mut []).unwrap();
        }
        let ours = ours.finish().unwrap();
        // Test-only raw split verifies complete key agreement, including PSK,
        // context, X25519 and both directions, against the independent Noise engine.
        let (first, second) = other.dangerously_get_raw_split();
        let (tx, rx) = if initiator {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(ours.tx.as_slice(), tx);
        assert_eq!(ours.rx.as_slice(), rx);
    }
}
#[test]
fn records_allow_cancel_gaps_but_reject_replay_tampering_and_reflection() {
    let (mut a, mut b) = pair();
    let mut wire = [0; 80];
    let mut out = [0; 64];
    a.seal(1, b"partial run abandoned", &mut wire).unwrap();
    let n = a.seal(2, b"cancel", &mut wire).unwrap();
    assert_eq!(b.open(2, &wire[..n], &mut out), Ok(6));
    assert_eq!(&out[..6], b"cancel");
    assert_eq!(b.open(2, &wire[..n], &mut out), Err(Error::Sequence));
    assert!(a.open(2, &wire[..n], &mut out).is_err());
    let n = a.seal(3, b"request", &mut wire).unwrap();
    wire[0] ^= 1;
    assert_eq!(b.open(3, &wire[..n], &mut out), Err(Error::Authentication));
    assert_eq!(&out[..7], &[0; 7]);
    wire[0] ^= 1;
    assert!(b.open(4, &wire[..n], &mut out).is_err());
    assert_eq!(b.open(3, &wire[..n], &mut out), Ok(7));
    let n = b.seal(1, b"response", &mut wire).unwrap();
    assert_eq!(a.open(1, &wire[..n], &mut out), Ok(8));
    assert!(a.seal(3, b"duplicate", &mut wire).is_err());
}
#[test]
fn incorrect_psk_context_and_malformed_handshakes_fail_closed() {
    for wrong_context in [true, false] {
        let profile = Profile::new(ID, KEY).unwrap();
        let bad = Profile::new(
            if wrong_context { [9; 16] } else { ID },
            if wrong_context { KEY } else { [9; 32] },
        )
        .unwrap();
        let mut a = Handshake::new(true, &profile, [11; 32]);
        let mut b = Handshake::new(false, &bad, [12; 32]);
        let mut message = [0; HANDSHAKE_LEN];
        a.write(&mut message).unwrap();
        assert_eq!(b.read(&message), Err(Error::Authentication));
        assert!(b.finish().is_err());
    }
    let profile = Profile::new(ID, KEY).unwrap();
    let mut b = Handshake::new(false, &profile, [12; 32]);
    assert_eq!(b.read(&[0; 47]), Err(Error::Length));
    assert!(b.finish().is_err());
}
#[test]
fn maximum_records_short_buffers_and_previous_sessions() {
    let (mut a, mut b) = pair();
    let (_, mut different) = {
        let profile = Profile::new(ID, KEY).unwrap();
        let mut a = Handshake::new(true, &profile, [13; 32]);
        let mut b = Handshake::new(false, &profile, [14; 32]);
        let mut msg = [0; HANDSHAKE_LEN];
        a.write(&mut msg).unwrap();
        b.read(&msg).unwrap();
        b.write(&mut msg).unwrap();
        a.read(&msg).unwrap();
        (a.finish().unwrap(), b.finish().unwrap())
    };
    let input = [0x5a; MAX_PLAINTEXT];
    let mut wire = [0; MAX_CIPHERTEXT];
    let mut out = [0; MAX_PLAINTEXT];
    assert!(a.seal(1, &input, &mut wire[..MAX_CIPHERTEXT - 1]).is_err());
    assert_eq!(a.seal(1, &input, &mut wire), Ok(MAX_CIPHERTEXT));
    assert!(different.open(1, &wire, &mut out).is_err());
    assert_eq!(b.open(1, &wire, &mut out), Ok(MAX_PLAINTEXT));
    assert_eq!(out, input);
    assert!(b.open(2, &[0; 16], &mut out).is_err());
    assert!(a.seal(0, b"no", &mut wire).is_err());
}
#[test]
fn provisioning_is_versioned_and_rejects_corruption_and_defaults() {
    let profile = Profile::new(ID, KEY).unwrap();
    let bytes = profile.encode(1);
    let (seq, decoded) = Profile::decode(&bytes).unwrap();
    assert_eq!(seq, 1);
    assert_eq!(decoded.key(), &KEY);
    assert_eq!(decoded.device_id, ID);
    for len in 0..bytes.len() {
        assert!(Profile::decode(&bytes[..len]).is_none());
    }
    for index in [0, 4, 5, 8, 12, 16, 32, 63] {
        let mut bad = bytes;
        bad[index] ^= 1;
        assert!(Profile::decode(&bad).is_none());
    }
    assert!(Profile::new(ID, [0; 32]).is_none());
    assert!(Profile::new([0; 16], KEY).is_none());
}
