#![allow(unused)]

use std::str::FromStr;

use clap::{Parser, ValueEnum};
use futures::StreamExt;
use regex::Regex;
use serenity::all::{
    CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage, Event,
    MessageUpdateEvent,
};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;

#[derive(PartialEq, Clone, Copy, Debug)]
enum ThumbnailMode {
    Disabled,
    Enabled,
    OnlyLocked,
}

impl ToString for ThumbnailMode {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

impl FromStr for ThumbnailMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "disabled" => Ok(ThumbnailMode::Disabled),
            "enabled" => Ok(ThumbnailMode::Enabled),
            "onlylocked" => Ok(ThumbnailMode::OnlyLocked),
            _ => Err(anyhow::anyhow!("can't parse thumbnail mode")),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrandingTitle {
    title: String,
    original: bool,
    votes: isize,
    locked: bool,
    #[serde(rename = "UUID")]
    uuid: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrandingThumbnail {
    timestamp: Option<f32>,
    original: bool,
    votes: isize,
    locked: bool,
    #[serde(rename = "UUID")]
    uuid: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrandingResponse {
    titles: Vec<BrandingTitle>,
    thumbnails: Vec<BrandingThumbnail>,
    random_time: f32,
    video_duration: Option<f32>,
}

async fn get_thumbnail(vid_id: &str, timestamp: Option<f32>) -> anyhow::Result<Vec<u8>> {
    let part = match timestamp {
        None => String::default(),
        Some(timestamp) => format!("&time={}", timestamp),
    };

    let link = format!(
        "https://dearrow-thumb.ajay.app/api/v1/getThumbnail?videoID={}{}",
        vid_id, part
    );

    Ok(reqwest::get(&link)
        .await?
        .bytes()
        .await?
        .into_iter()
        .collect::<Vec<_>>())
}

async fn get_branding(vid_id: &str) -> anyhow::Result<BrandingResponse> {
    let req = reqwest::get(&format!(
        "https://sponsor.ajay.app/api/branding?videoID={}",
        vid_id
    ))
    .await?;

    let res: BrandingResponse = req.json().await?;

    Ok(res)
}

struct Handler {
    remove_embed: bool,
    thumbnail_mode: ThumbnailMode,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, mut msg: Message) {
        let regex =
            Regex::new(r#"(?:youtube(?:-nocookie)?\.com\/(?:[^\/\n\s]+\/\S+\/|(?:v|e(?:mbed)?)\/|\S*?[?&]v=)|youtu\.be\/)([a-zA-Z0-9_-]{11})"#)
                .expect("failed to compile regex");

        let link = msg.content_safe(ctx.cache);

        let Some(cap) = regex.captures(&link) else {
            // log::warn!("regex did not capture");
            return;
        };

        let Some(id) = cap.get(1) else {
            log::warn!("link seemingly does not contain youtube id: {}", link);
            return;
        };

        let id = id.as_str().to_string();

        log::info!("de-clickbaiting {id}!");

        let Ok(branding) = get_branding(&id)
            .await
            .map_err(|e| log::error!("failed to get branding! {e:#?}"))
        else {
            return;
        };

        let Some(title) = branding.titles.first() else {
            log::warn!("no brandings returned!");
            return;
        };

        if !title.locked && title.votes < 0 {
            log::warn!(
                "untrusted branding (locked: {}, votes: {}). skipping.",
                title.locked,
                title.votes
            );
            return;
        }

        // if title.original {
        //     log::warn!("title is just recapitalized, skipping.");
        //     return;
        // }

        let thumb = if self.thumbnail_mode != ThumbnailMode::Disabled {
            match branding.thumbnails.first() {
                Some(thumbnail) => {
                    if !thumbnail.locked && thumbnail.votes < 0 {
                        log::warn!(
                            "untrusted thumbnail (locked: {}, votes: {}). skipping.",
                            title.locked,
                            title.votes
                        );
                        None
                    } else if !thumbnail.locked && self.thumbnail_mode == ThumbnailMode::OnlyLocked
                    {
                        log::warn!("only locked thumbnails allowed.");

                        None
                    } else {
                        get_thumbnail(&id, thumbnail.timestamp)
                            .await
                            .map_err(|e| log::error!("failed to retrieve thumbnail: {e:#?}"))
                            .ok()
                            .map(|x| (x, thumbnail.votes, thumbnail.locked))
                    }
                }
                None => {
                    log::warn!("no thumbnails returned!");
                    None
                }
            }
        } else {
            None
        };

        let message = CreateMessage::new();

        let thumb_present = thumb.is_some();

        let message = match thumb {
            Some((thumb, votes, locked)) => message
                .add_file(CreateAttachment::bytes(thumb, "thumb.webp"))
                .add_embed(
                    CreateEmbed::new()
                        .attachment("thumb.webp")
                        .title(&title.title)
                        .description(&format!(
                            "Title: {} votes, is{}locked; Thumbnail: {} votes, is{}locked",
                            title.votes,
                            if title.locked { " " } else { " not " },
                            votes,
                            if locked { " " } else { " not " }
                        ))
                        .footer(CreateEmbedFooter::new(
                            "De-Clickbait provided by DeArrow API.",
                        )),
                ),
            None => message.add_embed(
                CreateEmbed::new()
                    .title(&title.title)
                    .description(&format!(
                        "Title: {} votes, is{}locked; Thumbnail: {}",
                        title.votes,
                        if title.locked { " " } else { " not " },
                        match self.thumbnail_mode {
                            ThumbnailMode::Disabled => "disabled by dev",
                            ThumbnailMode::Enabled => "not found",
                            ThumbnailMode::OnlyLocked => "disabled by dev (lock-only)",
                        }
                    ))
                    .footer(CreateEmbedFooter::new(
                        "De-Clickbait provided by DeArrow API.",
                    )),
            ),
        }
        .reference_message(&msg);

        log::info!("Successfully generated de-clickbaited embed for {id}!");

        if let Err(e) = msg.channel_id.send_message(&ctx.http, message).await {
            log::error!("could not send message: {e:#?}");
        }

        if self.thumbnail_mode != ThumbnailMode::Disabled && thumb_present && self.remove_embed {
            if msg.embeds.len() == 0 {
                log::info!("waiting for discord to embed the video!");
                let msg_id = msg.id;

                let mut message_updates =
                    serenity::collector::collect(&ctx.shard, move |ev| match ev {
                        Event::MessageUpdate(x) if x.id == msg_id => Some(()),
                        _ => None,
                    });

                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(5000),
                    message_updates.next(),
                )
                .await;
            }

            log::info!("editing message to remove original embed!");

            if let Err(e) = msg
                .edit(&ctx.http, EditMessage::new().suppress_embeds(true))
                .await
            {
                log::error!("unable to edit sent message: {e:#?}");
            }
        }
    }
}

#[derive(Parser)]
struct Args {
    #[arg(long, env)]
    /// The discord token for the bot
    token: String,

    #[arg(long, env, default_value_t = ThumbnailMode::OnlyLocked)]
    /// The Thumbnail Mode. Indicates whether or not thumbnails should be embedded. "Locked" here refers to a crowd-sourced thumbnail having reached consensus status.
    thumbnail_mode: ThumbnailMode,

    #[arg(long, env)]
    /// Whether to remove the original embed from the sender.
    remove_embed: bool,
}

#[tokio::main]
async fn main() {
    colog::default_builder()
        .default_format()
        .filter(Some("serenity"), log::LevelFilter::Warn)
        .filter(Some("serenity"), log::LevelFilter::Warn)
        .filter(Some("serenity"), log::LevelFilter::Warn)
        .filter(Some("tracing::span"), log::LevelFilter::Warn)
        .filter_level(log::LevelFilter::Info)
        .init();

    #[cfg(debug_assertions)]
    {
        let token = include_str!("token");
        // wtf lmao why is this unsafe
        unsafe { std::env::set_var("TOKEN", token) };
    }

    let args = Args::parse();

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    log::info!("creating client");

    let mut client = Client::builder(&args.token, intents)
        .event_handler(Handler {
            remove_embed: args.remove_embed,
            thumbnail_mode: args.thumbnail_mode,
        })
        .await
        .expect("failed to create client");

    if let Err(e) = client.start().await {
        log::error!("{e:?}");
    }
}
