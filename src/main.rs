use anyhow::{format_err, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use colored_json::to_colored_json_auto;
use futures::TryStreamExt;
use itertools::Itertools;
use log::{info, LevelFilter};
use pulsar::{ConsumerOptions, Pulsar, SubType, TokioExecutor};
use serde_json::{json, Value};
use std::collections::HashMap;
use structopt::StructOpt;
use termion::color;
use url::Url;

#[derive(StructOpt)]
struct Opts {
    #[structopt(long, default_value = "pulsar://127.0.0.1")]
    url: Url,
    #[structopt(subcommand)]
    command: Command,
}

#[derive(StructOpt)]
enum Command {
    Consume {
        #[structopt(long)]
        topic: String,

        #[structopt(long, short = "s", default_value = "pulsar-cli")]
        subscriber_name: String,

        #[structopt(long)]
        durable: bool,

        #[structopt(long)]
        json: bool,
        #[structopt(long)]
        shared: bool,

        #[structopt(long)]
        ack: bool,
    },

    Publish {
        #[structopt(long)]
        topic: String,

        #[structopt(long, short = "p", default_value = "pulsar-cli")]
        producer_name: String,

        #[structopt(long, default_value = "5s")]
        interval: humantime::Duration,

        #[structopt(long = "prop")]
        properties: Vec<String>,
    },
}

async fn entry_point(opts: Opts) -> Result<()> {
    match opts.command {
        Command::Consume {
            subscriber_name,
            topic,
            durable,
            json,
            shared,
            ack,
        } => {
            let builder = Pulsar::builder(opts.url.as_str(), TokioExecutor)
                .build()
                .await?
                .consumer()
                .with_subscription(subscriber_name)
                .with_subscription_type(if shared {
                    SubType::Shared
                } else {
                    SubType::Exclusive
                })
                .with_topic(topic)
                .with_options(ConsumerOptions {
                    durable: Some(durable),
                    ..Default::default()
                });

            let mut consumer = builder.build::<Vec<u8>>().await?;

            while let Some(message) = consumer.try_next().await? {
                let publish_time = message.metadata().publish_time;
                let publish_time = DateTime::<Utc>::from_utc(
                    NaiveDateTime::from_timestamp(
                        (publish_time / 1000) as i64,
                        ((publish_time % 1000) * 1_000_000) as u32,
                    ),
                    Utc,
                );
                println!("-- {}:", publish_time);
                if !message.metadata().properties.is_empty() {
                    for item in message.metadata().properties.iter() {
                        println!(
                            "{}{}={}{}",
                            color::Fg(color::Magenta),
                            item.key,
                            item.value,
                            color::Fg(color::Reset)
                        );
                    }
                }
                if json {
                    match serde_json::from_slice::<Value>(&message.payload.data) {
                        Ok(val) => println!("{}", to_colored_json_auto(&val).unwrap()),
                        Err(_) => eprintln!(
                            "{}Value {:?} is not JSON{}",
                            color::Fg(color::Red),
                            String::from_utf8_lossy(&message.payload.data),
                            color::Fg(color::Reset)
                        ),
                    }
                } else {
                    println!("{}", String::from_utf8_lossy(&message.payload.data));
                }
                if ack {
                    consumer.ack(&message).await?;
                }
            }
            Ok(())
        }

        Command::Publish {
            topic,
            producer_name,
            interval,
            properties,
        } => {
            let properties = properties
                .iter()
                .map(|attr| {
                    let (key, value) = attr
                        .splitn(2, '=')
                        .tuples()
                        .next()
                        .ok_or_else(|| format_err!("Invalid attr: {:?}", attr))?;
                    Ok((key.to_owned(), value.to_owned()))
                })
                .collect::<Result<HashMap<_, _>>>()?;

            let mut producer = Pulsar::builder(opts.url.as_str(), TokioExecutor)
                .build()
                .await?
                .producer()
                .with_topic(topic)
                .with_name(producer_name)
                .build()
                .await?;
            info!("Connected to Pulsar");
            for i in 0.. {
                tokio::time::delay_for(interval.into()).await;
                let payload = serde_json::to_vec(&json!({
                    "iteration": i,
                    "timestamp": Utc::now(),
                }))?;
                let properties = properties.clone();

                let message = pulsar::producer::Message {
                    payload,
                    properties,
                    ..Default::default()
                };

                producer.send(message).await?;
                info!("Published message #{}", i);
            }
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() {
    let opts = Opts::from_args();
    env_logger::Builder::new()
        .filter_level(LevelFilter::Debug)
        .init();

    entry_point(opts).await.expect("Error encountered")
}
