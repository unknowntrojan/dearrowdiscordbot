#![allow(unused)]

use futures::StreamExt;
use regex::Regex;
use serenity::all::{
    CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage, Event,
    MessageUpdateEvent,
};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;

#[derive(PartialEq)]
enum ThumbnailMode {
    Disabled,
    Enabled,
    OnlyLocked,
}

const REMOVE_ORIGINAL: bool = false;
const THUMBNAIL_MODE: ThumbnailMode = ThumbnailMode::Enabled;

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

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message_update(
        &self,
        ctx: Context,
        old: Option<Message>,
        new: Option<Message>,
        event: MessageUpdateEvent,
    ) {
        log::info!("MessageUpdateEvent");
    }

    async fn message(&self, ctx: Context, mut msg: Message) {
        let regex =
            Regex::new(r#"(?:youtube(?:-nocookie)?\.com\/(?:[^\/\n\s]+\/\S+\/|(?:v|e(?:mbed)?)\/|\S*?[?&]v=)|youtu\.be\/)([a-zA-Z0-9_-]{11})"#)
                .expect("failed to compile regex");

        let link = msg.content_safe(ctx.cache);

        let Some(cap) = regex.captures(&link) else {
            log::warn!("regex did not capture");
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

        let thumb = if THUMBNAIL_MODE != ThumbnailMode::Disabled {
            match branding.thumbnails.first() {
                Some(thumbnail) => {
                    if !thumbnail.locked && thumbnail.votes < 0 {
                        log::warn!(
                            "untrusted thumbnail (locked: {}, votes: {}). skipping.",
                            title.locked,
                            title.votes
                        );
                        None
                    } else if !thumbnail.locked && THUMBNAIL_MODE == ThumbnailMode::OnlyLocked {
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
                        match THUMBNAIL_MODE {
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

        if THUMBNAIL_MODE != ThumbnailMode::Disabled && thumb_present && REMOVE_ORIGINAL {
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

    let token = include_str!("token");

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    log::info!("creating client");

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler)
        .await
        .expect("failed to create client");

    if let Err(e) = client.start().await {
        log::error!("{e:?}");
    }
}
