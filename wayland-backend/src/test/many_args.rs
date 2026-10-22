use std::{
    io,
    os::unix::io::AsFd,
    sync::atomic::{AtomicBool, Ordering},
};

use super::*;

struct ServerData(AtomicBool);

macro_rules! serverdata_impls {
    ($server_backend:tt) => {
        impl $server_backend::ObjectData<()> for ServerData {
            fn request(
                self: Arc<Self>,
                _: &$server_backend::Handle,
                _: &mut (),
                _: &$server_backend::ClientId,
                msg: OwnedMessage<$server_backend::ObjectId>,
            ) -> Option<Arc<dyn $server_backend::ObjectData<()>>> {
                assert_eq!(msg.opcode, 0);
                if let [
                    OwnedArgument::Uint(u),
                    OwnedArgument::Int(i),
                    OwnedArgument::Fixed(f),
                    OwnedArgument::Array(a),
                    OwnedArgument::Str(Some(s)),
                    OwnedArgument::Fd(fd),
                ] = &msg.args[..]
                {
                    assert_eq!(*u, 42);
                    assert_eq!(*i, -13);
                    assert_eq!(*f, 4589);
                    assert_eq!(&**a, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
                    assert_eq!(&***s, "I like trains");
                    // compare the fd to stdin
                    let stat1 = rustix::fs::fstat(&fd).unwrap();
                    let stat2 = rustix::fs::fstat(std::io::stdin()).unwrap();
                    assert_eq!(stat1.st_dev, stat2.st_dev);
                    assert_eq!(stat1.st_ino, stat2.st_ino);
                } else {
                    panic!("Bad argument list !")
                }
                self.0.store(true, Ordering::SeqCst);
                None
            }

            fn destroyed(
                self: Arc<Self>,
                _: &$server_backend::Handle,
                _: &mut (),
                _: &$server_backend::ClientId,
                _: &$server_backend::ObjectId,
            ) {
            }
        }

        impl $server_backend::GlobalHandler<()> for ServerData {
            fn bind(
                self: Arc<Self>,
                handle: &$server_backend::Handle,
                _: &mut (),
                _: &$server_backend::ClientId,
                _: &$server_backend::GlobalId,
                object_id: &$server_backend::ObjectId,
            ) -> Arc<dyn $server_backend::ObjectData<()>> {
                let stdout = io::stdout().lock();
                handle
                    .send_event(message!(
                        &object_id,
                        0,
                        [
                            Argument::Uint(1337),
                            Argument::Int(-53),
                            Argument::Fixed(9823),
                            Argument::Array(Box::new(&[10, 20, 30, 40, 50, 60, 70, 80, 90])),
                            Argument::Str(Some(Box::new("I want cake"))),
                            Argument::Fd(stdout.as_fd()),
                        ],
                    ))
                    .unwrap();
                self
            }
        }
    };
}

serverdata_impls!(server_rs);
serverdata_impls!(server_sys);

struct ClientData(AtomicBool);

macro_rules! clientdata_impls {
    ($client_backend:tt) => {
        impl $client_backend::ObjectData for ClientData {
            fn event(
                self: Arc<Self>,
                _handle: &$client_backend::Backend,
                msg: OwnedMessage<$client_backend::ObjectId>,
            ) -> Option<Arc<dyn $client_backend::ObjectData>> {
                assert_eq!(msg.opcode, 0);
                if let [
                    OwnedArgument::Uint(u),
                    OwnedArgument::Int(i),
                    OwnedArgument::Fixed(f),
                    OwnedArgument::Array(a),
                    OwnedArgument::Str(Some(s)),
                    OwnedArgument::Fd(fd),
                ] = &msg.args[..]
                {
                    assert_eq!(*u, 1337);
                    assert_eq!(*i, -53);
                    assert_eq!(*f, 9823);
                    assert_eq!(&**a, &[10, 20, 30, 40, 50, 60, 70, 80, 90]);
                    assert_eq!(&***s, "I want cake");
                    // compare the fd to stdout
                    let stat1 = rustix::fs::fstat(&fd).unwrap();
                    let stat2 = rustix::fs::fstat(std::io::stdout()).unwrap();
                    assert_eq!(stat1.st_dev, stat2.st_dev);
                    assert_eq!(stat1.st_ino, stat2.st_ino);
                } else {
                    panic!("Bad argument list !")
                }
                self.0.store(true, Ordering::SeqCst);
                None
            }
            fn destroyed(&self, _object_id: &$client_backend::ObjectId) {}
        }
    };
}

clientdata_impls!(client_rs);
clientdata_impls!(client_sys);

// create a global and send the many_args method
expand_test!(many_args, {
    let (tx, rx) = std::os::unix::net::UnixStream::pair().unwrap();
    let server = server_backend::Backend::new().unwrap();
    let _client_id = server.handle().insert_client(rx, Arc::new(())).unwrap();
    let client = client_backend::Backend::connect(tx).unwrap();

    let server_data = Arc::new(ServerData(AtomicBool::new(false)));
    let client_data = Arc::new(ClientData(AtomicBool::new(false)));

    // Prepare a global
    server.handle().create_global(&interfaces::TEST_GLOBAL_INTERFACE, 1, server_data.clone());

    // get the registry client-side
    let client_display = client.display_id();
    let registry_id = client
        .send_request(
            message!(&client_display, 1, [Argument::NewId(client_backend::ObjectId::null())],),
            Some(Arc::new(DoNothingData)),
            Some((&interfaces::WL_REGISTRY_INTERFACE, 1)),
        )
        .unwrap();
    // create the test global
    let test_global_id = client
        .send_request(
            message!(
                &registry_id,
                0,
                [
                    Argument::Uint(1),
                    Argument::Str(Some(Box::new(interfaces::TEST_GLOBAL_INTERFACE.name))),
                    Argument::Uint(1),
                    Argument::NewId(client_backend::ObjectId::null()),
                ],
            ),
            Some(client_data.clone()),
            Some((&interfaces::TEST_GLOBAL_INTERFACE, 1)),
        )
        .unwrap();

    client.flush().unwrap();
    server.dispatch_all_clients(&mut ()).unwrap();
    server.flush(None).unwrap();
    client.prepare_read().unwrap().read().unwrap();
    assert!(client_data.0.load(Ordering::SeqCst));

    // send the many_args request
    let stdin = io::stdin().lock();
    client
        .send_request(
            message!(
                &test_global_id,
                0,
                [
                    Argument::Uint(42),
                    Argument::Int(-13),
                    Argument::Fixed(4589),
                    Argument::Array(Box::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9])),
                    Argument::Str(Some(Box::new("I like trains"))),
                    Argument::Fd(stdin.as_fd()),
                ],
            ),
            None,
            None,
        )
        .unwrap();
    client.flush().unwrap();

    server.dispatch_all_clients(&mut ()).unwrap();

    assert!(server_data.0.load(Ordering::SeqCst));
});

// rs server wire trace of many_args, every arg type
#[cfg(feature = "log")]
#[test]
fn many_args_wire_trace() {
    use crate::debug::WIRE_CAPTURE;

    let (tx, rx) = std::os::unix::net::UnixStream::pair().unwrap();
    let server = server_rs::Backend::new().unwrap();
    let _client_id = server.handle().insert_client(rx, Arc::new(())).unwrap();
    let client = client_rs::Backend::connect(tx).unwrap();
    let server_data = Arc::new(ServerData(AtomicBool::new(false)));
    let client_data = Arc::new(ClientData(AtomicBool::new(false)));
    server.handle().create_global(&interfaces::TEST_GLOBAL_INTERFACE, 1, server_data.clone());

    WIRE_CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
    let client_display = client.display_id();
    let registry_id = client
        .send_request(
            message!(&client_display, 1, [Argument::NewId(client_rs::ObjectId::null())],),
            Some(Arc::new(DoNothingData)),
            Some((&interfaces::WL_REGISTRY_INTERFACE, 1)),
        )
        .unwrap();
    let test_global_id = client
        .send_request(
            message!(
                &registry_id,
                0,
                [
                    Argument::Uint(1),
                    Argument::Str(Some(Box::new(interfaces::TEST_GLOBAL_INTERFACE.name))),
                    Argument::Uint(1),
                    Argument::NewId(client_rs::ObjectId::null()),
                ],
            ),
            Some(client_data.clone()),
            Some((&interfaces::TEST_GLOBAL_INTERFACE, 1)),
        )
        .unwrap();
    client.flush().unwrap();
    server.dispatch_all_clients(&mut ()).unwrap();
    server.flush(None).unwrap();
    client.prepare_read().unwrap().read().unwrap();
    let stdin = io::stdin().lock();
    client
        .send_request(
            message!(
                &test_global_id,
                0,
                [
                    Argument::Uint(42),
                    Argument::Int(-13),
                    Argument::Fixed(4589),
                    Argument::Array(Box::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9])),
                    Argument::Str(Some(Box::new("I like trains"))),
                    Argument::Fd(stdin.as_fd()),
                ],
            ),
            None,
            None,
        )
        .unwrap();
    client.flush().unwrap();
    server.dispatch_all_clients(&mut ()).unwrap();
    assert!(server_data.0.load(Ordering::SeqCst));

    let lines = WIRE_CAPTURE.with(|c| c.borrow_mut().take()).unwrap();
    let lines: Vec<&str> = lines.iter().map(|l| crate::debug::tests::untimed(l)).collect();
    let conn = lines[0].split_once(' ').unwrap().0;
    assert!(conn.starts_with('<') && conn.ends_with('>'), "{}", lines[0]);
    let expected = [
        "wl_display#1.get_registry(new id wl_registry#2)".to_string(),
        " -> wl_registry#2.global(1, \"test_global\", 1)".to_string(),
        "wl_registry#2.bind(1, \"test_global\", 1, new id [unknown]#3)".to_string(),
        " -> test_global#3.many_args_evt(1337, -53, 38.37109375, array[9], \"I want cake\", fd "
            .to_string(),
        "test_global#3.many_args(42, -13, 17.92578125, array[9], \"I like trains\", fd "
            .to_string(),
    ];
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, want) in lines.iter().zip(&expected) {
        let rest = line.strip_prefix(conn).and_then(|l| l.strip_prefix(' ')).unwrap();
        assert!(rest.starts_with(want.as_str()), "{rest:?} is not {want:?}");
    }
}
