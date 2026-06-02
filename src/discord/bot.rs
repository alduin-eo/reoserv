use chrono::Utc;
use std::cmp;

use serenity::{
    async_trait,
    builder::{
        CreateActionRow, CreateButton, CreateCommand, CreateCommandOption, CreateEmbed,
        CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage,
        CreateMessage, CreateModal, EditMessage,
    },
    client::{Client, Context, EventHandler},
    model::{
        Colour,
        application::{
            ActionRowComponent, ButtonStyle, Command, CommandDataOptionValue, CommandOptionType,
            InputTextStyle, Interaction,
        },
        gateway::Ready,
        id::{ChannelId, GuildId, MessageId, RoleId},
    },
    prelude::GatewayIntents,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    SETTINGS,
    character::Character,
    db::{DbHandle, insert_params},
    discord::DiscordCommand,
    resolve_transaction::resolve_transaction,
    world::WorldHandle,
};

use eolib::protocol::net::{
    PacketAction, PacketFamily,
    server::{
        AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
        AlduinReplyServerPacketReplyDataNotify, TransactionStatus,
    },
};

fn format_tx_status(status_id: i32) -> &'static str {
    match status_id {
        0 => "Pending",
        1 => "Approved",
        2 => "Cancelled",
        _ => "Unknown",
    }
}

fn format_action(action_id: i32) -> &'static str {
    if action_id == 0 {
        "Deposit"
    } else {
        "Withdraw"
    }
}

fn user_discord_name(user: &serenity::model::user::User) -> String {
    user.name.clone()
}

struct BotState {
    db: DbHandle,
    world: WorldHandle,
    guild_id: GuildId,
    channel_id: ChannelId,
    allowed_roles: Vec<RoleId>,
}

fn has_allowed_role(member_roles: &[RoleId], allowed_roles: &[RoleId]) -> bool {
    if allowed_roles.is_empty() {
        return true;
    }
    member_roles.iter().any(|r| allowed_roles.contains(r))
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, data_about_bot: Ready) {
        tracing::info!("Discord bot logged in as {}", data_about_bot.user.name);

        let state = {
            let data = ctx.data.read().await;
            let db = data.get::<Db>().unwrap().clone();
            let world = data.get::<Wrld>().unwrap().clone();
            let guild_id = *data.get::<Gld>().unwrap();
            let channel_id = *data.get::<Chnl>().unwrap();
            let allowed_roles = data.get::<Rols>().unwrap().clone();
            BotState {
                db,
                world,
                guild_id,
                channel_id,
                allowed_roles,
            }
        };

        if state.guild_id.get() == 0 {
            tracing::warn!("Discord guild_id not configured, skipping slash command registration");
            return;
        }

        let commands = vec![
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("approve")
                    .description("Approve a pending Alduin transaction")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "id",
                            "Transaction ID",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "reason",
                            "Optional reason",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("decline")
                    .description("Decline a pending Alduin transaction")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "id",
                            "Transaction ID",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "reason",
                            "Optional reason",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("transaction")
                    .description("Show details of a specific transaction")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "id",
                            "Transaction ID",
                        )
                        .required(true),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("transactions")
                    .description("List transactions with optional filters")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "status",
                            "Filter by status",
                        )
                        .add_string_choice("Pending", "pending")
                        .add_string_choice("Approved", "approved")
                        .add_string_choice("Cancelled", "cancelled")
                        .required(false),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "character",
                            "Filter by character name",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("give")
                    .description("Ad-hoc deposit of Alduin to a character")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "name",
                            "Character name",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "amount",
                            "Amount of Alduin",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "comment",
                            "Reason for the adjustment",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("take")
                    .description("Ad-hoc withdrawal of Alduin from a character")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "name",
                            "Character name",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "amount",
                            "Amount of Alduin",
                        )
                        .required(true),
                    )
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "comment",
                            "Reason for the adjustment",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("leaderboard")
                    .description("Top characters by Alduin amount")
                    .add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "page",
                            "Page number (default 1)",
                        )
                        .required(false),
                    ),
            )
            .await,
            Command::create_global_command(
                &ctx.http,
                CreateCommand::new("pending")
                    .description("List pending transactions with message links"),
            )
            .await,
        ];

        for result in &commands {
            if let Err(e) = result {
                tracing::error!("Failed to register slash command: {}", e);
            }
        }
        tracing::info!("Registered {} slash commands", commands.len());

        if state.channel_id.get() == 0 {
            return;
        }

        let rows = state
            .db
            .query(&insert_params(
                "SELECT ct.id, c.name, ct.action_id, ct.amount, ct.wallet_address, ct.created_at \
                 FROM character_transaction ct \
                 JOIN characters c ON c.id = ct.character_id \
                 WHERE ct.status_id = 0 AND ct.sent_to_discord = 0",
                &[],
            ))
            .await
            .unwrap_or_default();

        for row in &rows {
            let tx_id = row.get_int(0).unwrap_or(0);
            let name = row.get_string(1).unwrap_or_default();
            let action_id = row.get_int(2).unwrap_or(0);
            let amount = row.get_int(3).unwrap_or(0);
            let wallet = row.get_string(4).unwrap_or_default();
            let created = row.get_int(5).unwrap_or(0);

            let action_str = format_action(action_id);
            let created_str = chrono::DateTime::from_timestamp(created as i64, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_default();

            let embed = CreateEmbed::new()
                .title(format!("Pending Transaction #{}", tx_id))
                .color(Colour(0xf1c40f))
                .field("Character", &name, true)
                .field("Action", action_str, true)
                .field("Amount", amount.to_string(), true)
                .field("Wallet", &wallet, false)
                .field("Created", &created_str, false);

            let send_result = state
                .channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().add_embed(embed).components(vec![
                        CreateActionRow::Buttons(vec![
                            CreateButton::new(format!("approve_{}", tx_id))
                                .style(ButtonStyle::Success)
                                .label("Approve"),
                            CreateButton::new(format!("decline_{}", tx_id))
                                .style(ButtonStyle::Danger)
                                .label("Decline"),
                        ]),
                    ]),
                )
                .await;

            match send_result {
                Ok(msg) => {
                    let message_id = msg.id.get().to_string();
                    let _ = state
                        .db
                        .execute(&insert_params(
                            "UPDATE character_transaction \
                             SET sent_to_discord = 1, discord_message_id = :msg_id \
                             WHERE id = :id",
                            &[("id", &tx_id), ("msg_id", &message_id)],
                        ))
                        .await;
                }
                Err(e) => {
                    tracing::error!("Failed to post pending tx #{} to Discord: {}", tx_id, e);
                    continue;
                }
            }
        }

        if !rows.is_empty() {
            tracing::info!(
                "Posted {} missed pending transactions to Discord",
                rows.len()
            );
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let state = {
            let data = ctx.data.read().await;
            let db = data.get::<Db>().unwrap().clone();
            let world = data.get::<Wrld>().unwrap().clone();
            let guild_id = *data.get::<Gld>().unwrap();
            let channel_id = *data.get::<Chnl>().unwrap();
            let allowed_roles = data.get::<Rols>().unwrap().clone();
            BotState {
                db,
                world,
                guild_id,
                channel_id,
                allowed_roles,
            }
        };

        match interaction {
            Interaction::Command(cmd) => {
                if let Some(member) = &cmd.member
                    && !has_allowed_role(&member.roles, &state.allowed_roles)
                {
                    let _ = cmd
                        .create_response(
                            &ctx.http,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content("You do not have permission to use this command.")
                                    .ephemeral(true),
                            ),
                        )
                        .await;
                    return;
                }
                handle_slash_command(&ctx, &state, cmd).await;
            }
            Interaction::Component(comp) => {
                if let Some(member) = &comp.member
                    && !has_allowed_role(&member.roles, &state.allowed_roles)
                {
                    let _ = comp
                        .create_response(
                            &ctx.http,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content("You do not have permission to use this action.")
                                    .ephemeral(true),
                            ),
                        )
                        .await;
                    return;
                }
                handle_component(&ctx, comp).await;
            }
            Interaction::Modal(modal) => {
                handle_modal(&ctx, &state, modal).await;
            }
            _ => {}
        }
    }
}

async fn handle_slash_command(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    match cmd.data.name.as_str() {
        "approve" => resolve_via_command(ctx, state, cmd, TransactionStatus::Approved).await,
        "decline" => resolve_via_command(ctx, state, cmd, TransactionStatus::Cancelled).await,
        "transaction" => show_transaction(ctx, state, cmd).await,
        "transactions" => list_transactions(ctx, state, cmd).await,
        "give" => cmd_give(ctx, state, cmd).await,
        "take" => cmd_take(ctx, state, cmd).await,
        "leaderboard" => cmd_leaderboard(ctx, state, cmd).await,
        "pending" => cmd_pending(ctx, state, cmd).await,
        _ => {
            let _ = cmd
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Unknown command")
                            .ephemeral(true),
                    ),
                )
                .await;
        }
    }
}

fn get_option_i64(
    cmd: &serenity::model::application::CommandInteraction,
    name: &str,
) -> Option<i64> {
    cmd.data
        .options
        .iter()
        .find(|opt| opt.name == name)
        .and_then(|opt| {
            if let CommandDataOptionValue::Integer(v) = &opt.value {
                Some(*v)
            } else {
                None
            }
        })
}

fn get_option_string(
    cmd: &serenity::model::application::CommandInteraction,
    name: &str,
) -> Option<String> {
    cmd.data
        .options
        .iter()
        .find(|opt| opt.name == name)
        .and_then(|opt| {
            if let CommandDataOptionValue::String(s) = &opt.value {
                Some(s.clone())
            } else {
                None
            }
        })
}

async fn resolve_via_command(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
    new_status: TransactionStatus,
) {
    let tx_id = get_option_i64(&cmd, "id").unwrap_or(0) as i32;
    let reason = get_option_string(&cmd, "reason");

    if tx_id <= 0 {
        let _ = cmd
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Invalid transaction ID.")
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    }

    let user_name = user_discord_name(&cmd.user);
    let comment = reason.clone().unwrap_or_default();

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    match resolve_transaction(
        &state.db,
        &state.world,
        tx_id,
        new_status,
        &user_name,
        &comment,
    )
    .await
    {
        Ok(result) => {
            let status_str = match result.new_status {
                TransactionStatus::Approved => "approved",
                TransactionStatus::Cancelled => "cancelled",
                _ => "resolved",
            };
            let msg = format!(
                "Transaction #{} {} successfully.\nCharacter: {}\nAction: {}\nAmount: {}\nOnline: {}",
                tx_id,
                status_str,
                result.character_name,
                format_action(result.action),
                result.amount,
                if result.was_online { "Yes" } else { "No" },
            );
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new().content(msg),
                )
                .await;
        }
        Err(e) => {
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new()
                        .content(format!("Error: {}", e)),
                )
                .await;
        }
    }
}

async fn show_transaction(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let tx_id = get_option_i64(&cmd, "id").unwrap_or(0) as i32;

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    if tx_id <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("Invalid transaction ID."),
            )
            .await;
        return;
    }

    let row = state
        .db
        .query_one(&insert_params(
            "SELECT ct.id, c.name, ct.action_id, ct.amount, ct.wallet_address, \
             ct.status_id, ct.created_at, ct.resolved_at, ct.resolved_by_name, ct.comment \
             FROM character_transaction ct \
             JOIN characters c ON c.id = ct.character_id \
             WHERE ct.id = :id",
            &[("id", &tx_id)],
        ))
        .await
        .unwrap_or(None);

    match row {
        Some(row) => {
            let id = row.get_int(0).unwrap_or(0);
            let name = row.get_string(1).unwrap_or_default();
            let action_id = row.get_int(2).unwrap_or(0);
            let amount = row.get_int(3).unwrap_or(0);
            let wallet = row.get_string(4).unwrap_or_default();
            let status_id = row.get_int(5).unwrap_or(0);
            let created = row.get_int(6).unwrap_or(0);
            let resolved_at = row.get_int(7);
            let resolved_by = row.get_string(8).unwrap_or_default();
            let comment_text = row.get_string(9).unwrap_or_default();

            let created_str = chrono::DateTime::from_timestamp(created as i64, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_default();
            let resolved_str = resolved_at.and_then(|ts| {
                chrono::DateTime::from_timestamp(ts as i64, 0)
                    .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            });

            let color = match status_id {
                0 => Colour(0xf1c40f),
                1 => Colour(0x00ff00),
                _ => Colour(0xff0000),
            };
            let title = format!("Transaction #{} — {}", id, format_tx_status(status_id));

            let mut embed = CreateEmbed::new()
                .title(&title)
                .color(color)
                .field("Character", &name, true)
                .field("Action", format_action(action_id), true)
                .field("Amount", amount.to_string(), true)
                .field("Wallet", &wallet, false)
                .field("Created", &created_str, false);

            if let Some(ref rs) = resolved_str {
                embed = embed.field("Resolved At", rs, true);
            }
            if !resolved_by.is_empty() {
                embed = embed.field("Resolved By", &resolved_by, true);
            }
            if !comment_text.is_empty() {
                embed = embed.field("Comment", &comment_text, true);
            }

            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new().add_embed(embed),
                )
                .await;
        }
        None => {
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new()
                        .content(format!("Transaction #{} not found.", tx_id)),
                )
                .await;
        }
    }
}

async fn list_transactions(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let status_filter = get_option_string(&cmd, "status");
    let character_filter = get_option_string(&cmd, "character");

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    let base_query = "\
        SELECT ct.id, c.name, ct.action_id, ct.amount, ct.status_id, ct.created_at \
        FROM character_transaction ct \
        JOIN characters c ON c.id = ct.character_id";

    let rows = match (status_filter.as_deref(), character_filter.as_deref()) {
        (Some(status), Some(name)) => {
            let status_id = match status {
                "pending" => 0,
                "approved" => 1,
                "cancelled" => 2,
                _ => 0,
            };
            state
                .db
                .query(&insert_params(
                    &format!("{} WHERE ct.status_id = :s AND c.name LIKE :n ORDER BY ct.created_at DESC LIMIT 25", base_query),
                    &[("s", &status_id), ("n", &format!("%{}%", name))],
                ))
                .await
                .unwrap_or_default()
        }
        (Some(status), None) => {
            let status_id = match status {
                "pending" => 0,
                "approved" => 1,
                "cancelled" => 2,
                _ => 0,
            };
            state
                .db
                .query(&insert_params(
                    &format!(
                        "{} WHERE ct.status_id = :s ORDER BY ct.created_at DESC LIMIT 25",
                        base_query
                    ),
                    &[("s", &status_id)],
                ))
                .await
                .unwrap_or_default()
        }
        (None, Some(name)) => state
            .db
            .query(&insert_params(
                &format!(
                    "{} WHERE c.name LIKE :n ORDER BY ct.created_at DESC LIMIT 25",
                    base_query
                ),
                &[("n", &format!("%{}%", name))],
            ))
            .await
            .unwrap_or_default(),
        (None, None) => state
            .db
            .query(&format!(
                "{} ORDER BY ct.created_at DESC LIMIT 25",
                base_query
            ))
            .await
            .unwrap_or_default(),
    };

    if rows.is_empty() {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new().content("No transactions found."),
            )
            .await;
        return;
    }

    let mut lines: Vec<String> = Vec::new();
    for row in &rows {
        let id = row.get_int(0).unwrap_or(0);
        let name = row.get_string(1).unwrap_or_default();
        let action_id = row.get_int(2).unwrap_or(0);
        let amount = row.get_int(3).unwrap_or(0);
        let status_id = row.get_int(4).unwrap_or(0);

        lines.push(format!(
            "#{} | {} | {} | {} | {}",
            id,
            name,
            format_action(action_id),
            amount,
            format_tx_status(status_id),
        ));
    }

    let content = format!("**Transactions:**\n{}", lines.join("\n"));
    let _ = cmd
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new().content(content),
        )
        .await;
}

async fn cmd_give(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let name = get_option_string(&cmd, "name").unwrap_or_default();
    let amount = get_option_i64(&cmd, "amount").unwrap_or(0) as i32;
    let comment = get_option_string(&cmd, "comment").unwrap_or_default();
    let user_name = user_discord_name(&cmd.user);

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    if name.is_empty() || amount <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("Invalid name or amount."),
            )
            .await;
        return;
    }

    let char_row = state
        .db
        .query_one(&insert_params(
            "SELECT id FROM characters WHERE name = :name",
            &[("name", &name)],
        ))
        .await
        .unwrap_or(None);

    let character_id = match char_row {
        Some(r) => r.get_int(0).unwrap_or(0),
        None => {
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new()
                        .content(format!("Character '{}' not found.", name)),
                )
                .await;
            return;
        }
    };

    if character_id <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new().content("Character not found."),
            )
            .await;
        return;
    }

    let alduin_item_id = SETTINGS.load().alduin.alduin_item_id;
    if alduin_item_id <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("Alduin item ID not configured."),
            )
            .await;
        return;
    }

    let now = chrono::Utc::now().timestamp() as i32;

    let _ = state
        .db
        .execute(&insert_params(
            "UPDATE character_transaction \
             SET status_id = 2, resolved_at = :now, resolved_by_name = :by, comment = :comment \
             WHERE character_id = :cid AND action_id = 0 AND status_id = 0",
            &[
                ("now", &now),
                ("by", &user_name),
                (
                    "comment",
                    &format!("Auto-cancelled for adhoc deposit: {}", comment),
                ),
                ("cid", &character_id),
            ],
        ))
        .await;

    let was_online = state.world.get_character_by_name(&name).await.is_ok();

    let mut actual_given = amount;
    if was_online {
        if let Ok(character) = state.world.get_character_by_name(&name).await {
            let player_id = character.player_id.unwrap_or(0);
            let map_id = character.map_id;
            let max_item = SETTINGS.load().limits.max_item;
            let current = character.get_item_amount(alduin_item_id);
            let capped = cmp::min(max_item - current, amount);
            actual_given = capped;
            if capped > 0
                && let Ok(map) = state.world.get_map(map_id).await
            {
                map.give_item(player_id, alduin_item_id, capped);
            }
            let balance = current + actual_given;
            let notify_packet = AlduinReplyServerPacket {
                reply: AlduinReply::Notify,
                reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                    AlduinReplyServerPacketReplyDataNotify {
                        transaction_id: 0,
                        status: TransactionStatus::Approved,
                        transaction_amount: amount,
                        total_alduin: balance,
                    },
                )),
            };
            if let Some(player) = character.player.as_ref() {
                player.send(PacketAction::Reply, PacketFamily::Alduin, &notify_packet);
            }
        }
    } else {
        if let Ok(mut character) = Character::load(&state.db, character_id).await {
            let max_item = SETTINGS.load().limits.max_item;
            let current = character.get_item_amount(alduin_item_id);
            let capped = cmp::min(max_item - current, amount);
            actual_given = capped;
            if capped > 0 {
                character.add_item_no_quest_rules(alduin_item_id, capped);
            }
            let _ = character.update(&state.db).await;
        }
    }

    let _ = state
        .db
        .execute(&insert_params(
            "INSERT INTO character_transaction \
             (character_id, action_id, amount, settled_amount, wallet_address, status_id, \
              created_at, resolved_at, notified, resolved_by_name, comment) \
             VALUES (:cid, 0, :amount, :settled, '', 1, :now, :now, 1, :by, :comment)",
            &[
                ("cid", &character_id),
                ("amount", &amount),
                ("settled", &actual_given),
                ("now", &now),
                ("by", &user_name),
                ("comment", &comment),
            ],
        ))
        .await;

    let _ = cmd
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new().content(format!(
                "Gave {} Alduin to {} (settled {}). Comment: {}",
                amount, name, actual_given, comment
            )),
        )
        .await;
}

async fn cmd_take(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let name = get_option_string(&cmd, "name").unwrap_or_default();
    let amount = get_option_i64(&cmd, "amount").unwrap_or(0) as i32;
    let comment = get_option_string(&cmd, "comment").unwrap_or_default();
    let user_name = user_discord_name(&cmd.user);

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    if name.is_empty() || amount <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("Invalid name or amount."),
            )
            .await;
        return;
    }

    let char_row = state
        .db
        .query_one(&insert_params(
            "SELECT id FROM characters WHERE name = :name",
            &[("name", &name)],
        ))
        .await
        .unwrap_or(None);

    let character_id = match char_row {
        Some(r) => r.get_int(0).unwrap_or(0),
        None => {
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new()
                        .content(format!("Character '{}' not found.", name)),
                )
                .await;
            return;
        }
    };

    if character_id <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new().content("Character not found."),
            )
            .await;
        return;
    }

    let alduin_item_id = SETTINGS.load().alduin.alduin_item_id;
    if alduin_item_id <= 0 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("Alduin item ID not configured."),
            )
            .await;
        return;
    }

    let now = chrono::Utc::now().timestamp() as i32;

    let _ = state
        .db
        .execute(&insert_params(
            "UPDATE character_transaction \
             SET status_id = 2, resolved_at = :now, resolved_by_name = :by, comment = :comment \
             WHERE character_id = :cid AND action_id = 1 AND status_id = 0",
            &[
                ("now", &now),
                ("by", &user_name),
                (
                    "comment",
                    &format!("Auto-cancelled for adhoc withdrawal: {}", comment),
                ),
                ("cid", &character_id),
            ],
        ))
        .await;

    let was_online = state.world.get_character_by_name(&name).await.is_ok();

    let mut actual_taken = amount;
    if was_online {
        if let Ok(character) = state.world.get_character_by_name(&name).await {
            let player_id = character.player_id.unwrap_or(0);
            let map_id = character.map_id;
            let current = character.get_item_amount(alduin_item_id);
            let capped = cmp::min(amount, current);
            actual_taken = capped;
            if capped > 0
                && let Ok(map) = state.world.get_map(map_id).await
            {
                map.lose_item(player_id, alduin_item_id, capped);
            }
            let balance = current - actual_taken;
            let notify_packet = AlduinReplyServerPacket {
                reply: AlduinReply::Notify,
                reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                    AlduinReplyServerPacketReplyDataNotify {
                        transaction_id: 0,
                        status: TransactionStatus::Approved,
                        transaction_amount: amount,
                        total_alduin: cmp::max(0, balance),
                    },
                )),
            };
            if let Some(player) = character.player.as_ref() {
                player.send(PacketAction::Reply, PacketFamily::Alduin, &notify_packet);
            }
        }
    } else {
        if let Ok(mut character) = Character::load(&state.db, character_id).await {
            let current = character.get_item_amount(alduin_item_id);
            let capped = cmp::min(amount, current);
            actual_taken = capped;
            if capped > 0 {
                character.remove_item_no_quest_rules(alduin_item_id, capped);
            }
            let _ = character.update(&state.db).await;
        }
    }

    let _ = state
        .db
        .execute(&insert_params(
            "INSERT INTO character_transaction \
             (character_id, action_id, amount, settled_amount, wallet_address, status_id, \
              created_at, resolved_at, notified, resolved_by_name, comment) \
             VALUES (:cid, 1, :amount, :settled, '', 1, :now, :now, 1, :by, :comment)",
            &[
                ("cid", &character_id),
                ("amount", &amount),
                ("settled", &actual_taken),
                ("now", &now),
                ("by", &user_name),
                ("comment", &comment),
            ],
        ))
        .await;

    let _ = cmd
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new().content(format!(
                "Took {} Alduin from {} (settled {}). Comment: {}",
                amount, name, actual_taken, comment
            )),
        )
        .await;
}

async fn cmd_leaderboard(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let page = cmp::max(1, get_option_i64(&cmd, "page").unwrap_or(1) as i32);
    let offset = (page - 1) * 10;

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    let alduin_item_id = SETTINGS.load().alduin.alduin_item_id;

    let rows = state
        .db
        .query(&insert_params(
            "SELECT c.name, COALESCE(SUM(ci.quantity), 0) AS total \
             FROM characters c \
             LEFT JOIN character_inventory ci ON ci.character_id = c.id AND ci.item_id = :item_id \
             GROUP BY c.id, c.name \
             ORDER BY total DESC \
             LIMIT 10 OFFSET :offset",
            &[("item_id", &alduin_item_id), ("offset", &offset)],
        ))
        .await
        .unwrap_or_default();

    if rows.is_empty() {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("No entries found on this page."),
            )
            .await;
        return;
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");

    let mut lines: Vec<String> = vec![format!(
        "**Alduin Leaderboard — Page {}** ({}):",
        page, timestamp
    )];
    for (rank, row) in (offset + 1..).zip(rows.iter()) {
        let name = row.get_string(0).unwrap_or_default();
        let total = row.get_int(1).unwrap_or(0);
        lines.push(format!("{}. {} — {}", rank, name, total));
    }

    let _ = cmd
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new().content(lines.join("\n")),
        )
        .await;
}

async fn cmd_pending(
    ctx: &Context,
    state: &BotState,
    cmd: serenity::model::application::CommandInteraction,
) {
    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    let rows = state
        .db
        .query(&insert_params(
            "SELECT ct.id, c.name, ct.action_id, ct.amount, ct.created_at, ct.discord_message_id \
             FROM character_transaction ct \
             JOIN characters c ON c.id = ct.character_id \
             WHERE ct.status_id = 0 \
             ORDER BY ct.created_at DESC",
            &[],
        ))
        .await
        .unwrap_or_default();

    if rows.is_empty() {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content("No pending transactions."),
            )
            .await;
        return;
    }

    let guild_id = state.guild_id.get();
    let channel_id = state.channel_id.get();
    let has_channel = guild_id != 0 && channel_id != 0;

    let mut lines: Vec<String> = Vec::new();
    for row in &rows {
        let id = row.get_int(0).unwrap_or(0);
        let name = row.get_string(1).unwrap_or_default();
        let action_id = row.get_int(2).unwrap_or(0);
        let amount = row.get_int(3).unwrap_or(0);
        let created = row.get_int(4).unwrap_or(0);
        let msg_id = row.get_string(5).unwrap_or_default();

        let created_str = chrono::DateTime::from_timestamp(created as i64, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();

        let link = if has_channel && !msg_id.is_empty() {
            format!(
                "https://discord.com/channels/{}/{}/{}",
                guild_id, channel_id, msg_id
            )
        } else {
            String::new()
        };

        let line = if link.is_empty() {
            format!(
                "#{} | {} | {} | {} | {}",
                id,
                name,
                format_action(action_id),
                amount,
                created_str
            )
        } else {
            format!(
                "#{} | {} | {} | {} | {} | {}",
                id,
                name,
                format_action(action_id),
                amount,
                created_str,
                link
            )
        };
        lines.push(line);
    }

    let content = format!("**Pending Transactions:**\n{}", lines.join("\n"));

    if content.len() > 2000 {
        let _ = cmd
            .edit_response(
                &ctx.http,
                serenity::builder::EditInteractionResponse::new()
                    .content(format!("**{} pending transactions** (content too long, use /transactions status:pending)", rows.len())),
            )
            .await;
        return;
    }

    let _ = cmd
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new().content(content),
        )
        .await;
}

async fn handle_component(ctx: &Context, comp: serenity::model::application::ComponentInteraction) {
    let custom_id = &comp.data.custom_id;
    if custom_id.starts_with("approve_") || custom_id.starts_with("decline_") {
        let parts: Vec<&str> = custom_id.split('_').collect();
        if parts.len() < 2 {
            return;
        }
        let tx_id: i32 = parts[1].parse().unwrap_or(0);
        if tx_id <= 0 {
            return;
        }

        let action = if custom_id.starts_with("approve_") {
            "approve"
        } else {
            "decline"
        };
        let modal_custom_id = format!("modal_{}_{}", action, tx_id);
        let modal_title = format!("{} Transaction #{}", action, tx_id);

        let modal = CreateModal::new(modal_custom_id, modal_title).components(vec![
            CreateActionRow::InputText(
                CreateInputText::new(InputTextStyle::Short, "Reason (optional)", "reason")
                    .placeholder("Optional reason...")
                    .required(false)
                    .max_length(200),
            ),
        ]);

        let _ = comp
            .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
            .await;
    }
}

async fn handle_modal(
    ctx: &Context,
    state: &BotState,
    modal: serenity::model::application::ModalInteraction,
) {
    let custom_id = &modal.data.custom_id;
    if !custom_id.starts_with("modal_approve_") && !custom_id.starts_with("modal_decline_") {
        return;
    }

    let parts: Vec<&str> = custom_id.split('_').collect();
    if parts.len() < 3 {
        return;
    }
    let action = parts[1];
    let tx_id: i32 = parts[2].parse().unwrap_or(0);
    if tx_id <= 0 {
        return;
    }

    let reason = modal
        .data
        .components
        .first()
        .and_then(|row| row.components.first())
        .and_then(|comp| match comp {
            ActionRowComponent::InputText(input) => input.value.clone(),
            _ => None,
        })
        .filter(|v| !v.is_empty());

    let new_status = match action {
        "approve" => TransactionStatus::Approved,
        "decline" => TransactionStatus::Cancelled,
        _ => return,
    };

    let user_name = user_discord_name(&modal.user);
    let comment = reason.clone().unwrap_or_default();

    let _ = modal
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    match resolve_transaction(
        &state.db,
        &state.world,
        tx_id,
        new_status,
        &user_name,
        &comment,
    )
    .await
    {
        Ok(result) => {
            let status_str = match result.new_status {
                TransactionStatus::Approved => "Approved",
                TransactionStatus::Cancelled => "Declined",
                _ => "Resolved",
            };

            let color = match result.new_status {
                TransactionStatus::Approved => Colour(0x00ff00),
                _ => Colour(0xff0000),
            };

            let title = format!("Transaction #{} — {}", tx_id, status_str);
            let reason_text = reason.as_deref().unwrap_or("None");

            let embed = CreateEmbed::new()
                .title(&title)
                .color(color)
                .field("Character", &result.character_name, true)
                .field("Action", format_action(result.action), true)
                .field("Amount", result.amount.to_string(), true)
                .field("Resolved By", &user_name, true)
                .field("Comment", reason_text, true);

            if state.channel_id.get() != 0 {
                let sent = state
                    .channel_id
                    .send_message(&ctx.http, CreateMessage::new().add_embed(embed))
                    .await;

                if sent.is_ok()
                    && let Some(ref orig_msg) = modal.message
                {
                    let _ = orig_msg.delete(&ctx.http).await;
                }
            }

            let _ = modal
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new().content(format!(
                        "Transaction #{} {} successfully!",
                        tx_id,
                        status_str.to_lowercase()
                    )),
                )
                .await;
        }
        Err(e) => {
            let _ = modal
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::new()
                        .content(format!("Error: {}", e)),
                )
                .await;
        }
    }
}

use serenity::prelude::TypeMapKey;

struct Db;
impl TypeMapKey for Db {
    type Value = DbHandle;
}

struct Wrld;
impl TypeMapKey for Wrld {
    type Value = WorldHandle;
}

struct Gld;
impl TypeMapKey for Gld {
    type Value = GuildId;
}

struct Chnl;
impl TypeMapKey for Chnl {
    type Value = ChannelId;
}

struct Rols;
impl TypeMapKey for Rols {
    type Value = Vec<RoleId>;
}

pub async fn spawn_bot(
    mut discord_rx: UnboundedReceiver<DiscordCommand>,
    db: DbHandle,
    world: WorldHandle,
) {
    let discord_config = SETTINGS.load().discord.clone();
    if !discord_config.enabled || discord_config.token.is_empty() {
        tracing::info!("Discord bot is disabled or not configured");
        while discord_rx.recv().await.is_some() {}
        return;
    }

    let guild_id = GuildId::new(discord_config.guild_id);
    let channel_id = ChannelId::new(discord_config.channel_id);
    let allowed_roles: Vec<RoleId> = discord_config
        .allowed_roles
        .iter()
        .map(|id| RoleId::new(*id))
        .collect();

    let token = discord_config.token.clone();

    let mut client = Client::builder(&token, GatewayIntents::non_privileged())
        .event_handler(Handler)
        .await
        .expect("Failed to create Discord client");

    {
        let mut data = client.data.write().await;
        data.insert::<Db>(db.clone());
        data.insert::<Wrld>(world.clone());
        data.insert::<Gld>(guild_id);
        data.insert::<Chnl>(channel_id);
        data.insert::<Rols>(allowed_roles);
    }

    let http = client.http.clone();

    tokio::spawn(async move {
        if let Err(e) = client.start().await {
            tracing::error!("Discord client error: {}", e);
        }
    });

    while let Some(cmd) = discord_rx.recv().await {
        match cmd {
            DiscordCommand::NewTransaction {
                tx_id,
                character_name,
                action,
                amount,
                wallet_address,
            } => {
                if channel_id.get() == 0 {
                    continue;
                }

                let action_str = format_action(action);
                let created_str = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

                let embed = CreateEmbed::new()
                    .title(format!("Pending Transaction #{}", tx_id))
                    .color(Colour(0xf1c40f))
                    .field("Character", &character_name, true)
                    .field("Action", action_str, true)
                    .field("Amount", amount.to_string(), true)
                    .field("Wallet", &wallet_address, false)
                    .field("Created", &created_str, false);

                let result = channel_id
                    .send_message(
                        &http,
                        CreateMessage::new().add_embed(embed).components(vec![
                            CreateActionRow::Buttons(vec![
                                CreateButton::new(format!("approve_{}", tx_id))
                                    .style(ButtonStyle::Success)
                                    .label("Approve"),
                                CreateButton::new(format!("decline_{}", tx_id))
                                    .style(ButtonStyle::Danger)
                                    .label("Decline"),
                            ]),
                        ]),
                    )
                    .await;

                match result {
                    Ok(msg) => {
                        let message_id = msg.id.get().to_string();
                        let _ = db
                            .execute(&insert_params(
                                "UPDATE character_transaction \
                                 SET sent_to_discord = 1, discord_message_id = :msg_id \
                                 WHERE id = :id",
                                &[("id", &tx_id), ("msg_id", &message_id)],
                            ))
                            .await;
                        tracing::debug!(
                            "Posted transaction #{} to Discord (msg {})",
                            tx_id,
                            message_id
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to post transaction #{} to Discord: {}", tx_id, e);
                    }
                }
            }
            DiscordCommand::TransactionCancelled { tx_id } => {
                let msg_id = db
                    .query_string(&insert_params(
                        "SELECT discord_message_id FROM character_transaction WHERE id = :id",
                        &[("id", &tx_id)],
                    ))
                    .await
                    .unwrap_or(None)
                    .and_then(|s| s.parse::<u64>().ok());

                if let Some(msg_id) = msg_id {
                    let embed = CreateEmbed::new()
                        .title(format!("Transaction #{} — Cancelled", tx_id))
                        .color(Colour(0xff0000))
                        .field("Note", "Self-cancelled by the player.", false);

                    let _ = channel_id
                        .edit_message(
                            &http,
                            MessageId::new(msg_id),
                            EditMessage::new().add_embed(embed).components(vec![]),
                        )
                        .await
                        .map_err(|e| {
                            tracing::error!("Failed to update cancelled tx #{} msg: {}", tx_id, e)
                        });
                }
            }
        }
    }
}
