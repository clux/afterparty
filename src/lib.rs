extern crate crypto;
extern crate hyper;
extern crate rustc_serialize;
extern crate time;

pub mod hook;
pub mod rep;

use crypto::mac::Mac;
use crypto::hmac::Hmac;
use crypto::sha1::Sha1;
use hook::Hook;
use hyper::server::{Handler, Server, Request, Response};
use rep::Payload;
use rustc_serialize::json;
use std::collections::HashMap;
use std::thread;
use std::io::Read;
use std::sync::Mutex;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Raw, unparsed representation of an inbound github event
#[derive(Clone, Debug, Default)]
pub struct Event {
  pub name: String,
  pub payload: String
}

/// A hub is a handler for github event requests
struct Hub {
  secret: Option<String>,
  deliveries: Mutex<Sender<Event>>,
  hooks: Vec<Hook>
}

impl Hub {
  pub fn authenticate(secret: &String, payload: &String, signature: &[u8]) -> bool {    
    let sbytes = secret.as_bytes();
    let pbytes = payload.as_bytes();
    let mut mac = Hmac::new(Sha1::new(), &sbytes);
    mac.input(&pbytes);
    mac.result().code().to_vec() == signature
  }
}

impl Handler for Hub {
  fn handle(&self, mut req: Request, res: Response) {
    let mut payload = String::new();
    if let Ok(_) = req.read_to_string(&mut payload) {
      if let Some(es) = req.headers.get_raw("X-Github-Event") {
        let event = String::from_utf8_lossy(&(*es[0])).to_string();
        println!("recv {} event", event);
        let deliver = || {
          let _ = self.deliveries.lock().unwrap().send(Event {
            name: event,
            payload: payload.clone()
          });
        };
        if let Some(ref secret) = self.secret {
           match req.headers.get_raw("X-Hub-Signature") {
              Some(sig) => {
                 if Hub::authenticate(&secret, &payload, &(*sig[0])) {
                   deliver()
                 } else {
                   println!("recv invalid signature for payload");
                 }
              },
                     _  => println!("recv unsigned request recieved")
           }
        } else {
          deliver()
        }
      }
    }
    let _ = res.send(b"ok");
    ()
  }
}

/// Filters filter inbound events, invoking interested hooks
struct Filter { hooks: Vec<Hook> }

impl Filter {

  /// filters hooks based on event name
  /// `*` will match any event
  pub fn filter(&self, ev: &Event, payload: &Payload) -> Vec<&Hook> {
    (*self.hooks).into_iter()
      .filter(|h| h.targets(&ev.name, payload))
      .collect::<Vec<&Hook>>()
  }

  /// recv's github events from rx and sends them
  /// to interested hooks.
  /// a thread will be spawned once for each hook
  /// before awaiting the reciept of events
  pub fn recv(&self, rx: Receiver<Event>) {
    let mut lookup     = HashMap::new();
    let mut deliveries = HashMap::new();
    for hook in (&self.hooks).iter() {
      lookup.insert((*hook).clone().name(), (*hook).clone());
    }
    for (name, hook) in lookup.into_iter() {
      let (tx, rx) = channel();
      deliveries.insert(name.clone(), tx);
      thread::spawn(move || {
        hook.recv(rx);
      });
    }
    while let Ok(ev) = rx.recv() {
      match json::decode::<Payload>(&ev.payload) {
        Ok(payload) => {
          println!("rec {} event", ev.name);
          let hooks = self.filter(&ev, &payload);
          for h in hooks {
            let name = h.clone().name();
            if let Err(e) = deliveries.get(&name).unwrap().send(ev.clone()) {
              println!("{} delivery to {} failed: {}", ev.name, name, e.to_string())
            }
          }
        }, _ => println!("rec unparsable payload {:?}", ev.payload)
      }
    }
  }
}

/// A Service serves inbound requests for github events
#[derive(Debug, RustcEncodable, RustcDecodable)]
pub struct Service {
  pub secret: Option<String>,
  pub port: Option<u16>,
  pub hooks: Vec<Hook>
}

impl Service {

  /// return the configured port or 4567
  pub fn port(&self) -> u16 {
    self.port.unwrap_or(4567)
  }

  /// sets the current list of hooks in memory
  pub fn hooks(&mut self, hooks: Vec<Hook>) {
    self.hooks = hooks;
  }

  /// starts listening on the configured port
  pub fn run(self) {
    let (tx, rx) = channel();
    let hooks = self.hooks.clone();
    thread::spawn(move || {
      let filter = Filter { hooks: hooks };
      filter.recv(rx)
    });   
    Server::http(&format!("0.0.0.0:{}", self.port())[..]).unwrap()
      .handle(Hub {
         secret: self.secret,
         deliveries: Mutex::new(tx),
         hooks: self.hooks
      }).unwrap();
  }
}

#[cfg(test)]
mod tests {
  use rustc_serialize::json;
  use super::{Event, Filter, Service};
  use super::hook::Hook;
  use super::rep::Payload;
  

  #[test]
  fn test_filter() {
    let hooks = vec![
      Hook {
        events: vec![
          "a".to_owned(),
          "b".to_owned()
        ],
        ..Default::default()
      },
      Hook {
        events: vec![
          "a".to_owned(),
          "c".to_owned()
        ],
        ..Default::default()
      },
      Hook {
        events: vec!["*".to_owned()],
        ..Default::default()
      }
    ];
    let filter = Filter { hooks: hooks };

    assert_eq!(3, filter.filter(&Event {
      name: "a".to_owned(),
      ..Default::default()
    }, &Payload {
       ..Default::default()
    }).len());
    
    assert_eq!(2, filter.filter(&Event {
      name: "c".to_owned(),
      ..Default::default()
    }, &Payload {
      ..Default::default()
    }).len())
  }

  #[test]
  fn test_service_config() {
    match json::decode::<Service>(&r#"{
     "hooks": [{
       "cmd": "ls",
       "events": ["test"]
     }]
    }"#) {
      Ok(svc) => {
         assert_eq!(4567, svc.port());
         assert_eq!(1, svc.hooks.len());
      },
      Err(e) => panic!("failed to decode service {}", e)
    }
  }
}