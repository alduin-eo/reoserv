use chrono::Utc;
use serenity::{
    async_trait,
    builder::{
        CreateActionRow, CreateButton, CreateCommand, CreateCommandOption, CreateEmbed,
        CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage,
        CreateMessage, CreateModal,
    },
    client::{Client, Context, EventHandler},
    model::{
        Colour,
        application::{
            ActionRowComponent, ButtonStyle, Command, CommandDataOptionValue, CommandOptionType,
            InputTextStyle, Interaction,
        },
        gateway::Ready,
        id::{ChannelId, GuildId, RoleId},
    },
    prelude::GatewayIntents,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    SETTINGS,
    db::{DbHandle, insert_params},
    discord::DiscordCommand,
    resolve_transaction::resolve_transaction,
    world::WorldHandle,
};

use eolib::protocol::net::server::TransactionStatus;

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

fn user_display_name(user: &serenity::model::user::User) -> String {
    user.global_name
        .as_deref()
        .unwrap_or(&user.name)
        .to_string()
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

            if let Err(e) = state
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
                .await
            {
                tracing::error!("Failed to post pending tx #{} to Discord: {}", tx_id, e);
                continue;
            }

            let _ = state
                .db
                .execute(&insert_params(
                    "UPDATE character_transaction SET sent_to_discord = 1 WHERE id = :id",
                    &[("id", &tx_id)],
                ))
                .await;
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

    let user_name = user_display_name(&cmd.user);
    let resolved_by = if let Some(ref r) = reason {
        format!("{} ({})", user_name, r)
    } else {
        user_name.clone()
    };

    let _ = cmd
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    match resolve_transaction(&state.db, &state.world, tx_id, new_status, &resolved_by).await {
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
             ct.status_id, ct.created_at, ct.resolved_at, ct.resolved_by_name \
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

    let mut where_clauses = Vec::new();
    let mut params: Vec<(&str, Box<dyn std::fmt::Debug + Send>)> = Vec::new();

    if let Some(ref status) = status_filter {
        let status_id: i32 = match status.as_str() {
            "pending" => 0,
            "approved" => 1,
            "cancelled" => 2,
            _ => 0,
        };
        where_clauses.push("ct.status_id = :status");
        params.push(("status", Box::new(status_id)));
    }

    if let Some(ref filter_name) = character_filter {
        where_clauses.push("c.name LIKE :name");
        params.push(("name", Box::new(format!("%{}%", filter_name))));
    }

    let where_clause = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let query = format!(
        "SELECT ct.id, c.name, ct.action_id, ct.amount, ct.status_id, ct.created_at \
         FROM character_transaction ct \
         JOIN characters c ON c.id = ct.character_id \
         {} \
         ORDER BY ct.created_at DESC LIMIT 25",
        where_clause
    );

    let rows = state.db.query(&query).await.unwrap_or_default();

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

    let user_name = user_display_name(&modal.user);
    let resolved_by = if let Some(ref r) = reason {
        format!("{} ({})", user_name, r)
    } else {
        user_name.clone()
    };

    let _ = modal
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await;

    match resolve_transaction(&state.db, &state.world, tx_id, new_status, &resolved_by).await {
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
                .field("Reason", reason_text, true);

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
                    Ok(_) => {
                        let _ = db
                            .execute(&insert_params(
                                "UPDATE character_transaction SET sent_to_discord = 1 WHERE id = :id",
                                &[("id", &tx_id)],
                            ))
                            .await;
                        tracing::debug!("Posted transaction #{} to Discord", tx_id);
                    }
                    Err(e) => {
                        tracing::error!("Failed to post transaction #{} to Discord: {}", tx_id, e);
                    }
                }
            }
        }
    }
}
