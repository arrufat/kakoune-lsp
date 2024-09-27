use std::collections::HashMap;

use crate::capabilities::{attempt_server_capability, CAPABILITY_RANGE_FORMATTING};
use crate::context::*;
use crate::controller::can_serve;
use crate::position::{kakoune_range_to_lsp, parse_kakoune_range};
use crate::text_edit::{apply_text_edits_to_buffer, TextEditish};
use crate::types::*;
use crate::util::editor_quote;
use itertools::Itertools;
use lsp_types::request::*;
use lsp_types::*;
use url::Url;

pub fn text_document_range_formatting(
    meta: EditorMeta,
    params: RangeFormattingParams,
    ctx: &mut Context,
) {
    let eligible_servers: Vec<_> = ctx
        .servers(&meta)
        .filter(|server| attempt_server_capability(*server, &meta, CAPABILITY_RANGE_FORMATTING))
        .filter(|(server_id, _)| {
            meta.server
                .as_ref()
                .map(|fmt_server| {
                    can_serve(
                        ctx,
                        *server_id,
                        fmt_server,
                        &server_configs(&ctx.config, &meta)[fmt_server].root,
                    )
                })
                .unwrap_or(true)
        })
        .collect();
    if eligible_servers.is_empty() {
        if meta.fifo.is_some() {
            ctx.exec(meta, "nop");
        }
        return;
    }

    // Ask user to pick which server to use for formatting when multiple options are available.
    if eligible_servers.len() > 1 {
        let choices = eligible_servers
            .into_iter()
            .map(|(_server_id, server)| {
                let cmd = if meta.fifo.is_some() {
                    "lsp-range-formatting-sync"
                } else {
                    "lsp-range-formatting"
                };
                let cmd = format!("{} {}", cmd, server.name);
                format!("{} {}", editor_quote(&server.name), editor_quote(&cmd))
            })
            .join(" ");
        ctx.exec(meta, format!("lsp-menu {}", choices));
        return;
    }

    let Some(document) = ctx.documents.get(&meta.buffile) else {
        warn!(
            meta.session,
            "No document in context for file: {}", &meta.buffile
        );
        return;
    };

    let (server_id, server) = eligible_servers[0];
    let mut req_params = HashMap::new();
    req_params.insert(
        server_id,
        params
            .ranges
            .iter()
            .map(|s| {
                let (range, _cursor) = parse_kakoune_range(s);
                kakoune_range_to_lsp(&range, &document.text, server.offset_encoding)
            })
            .map(|range| DocumentRangeFormattingParams {
                text_document: TextDocumentIdentifier {
                    uri: Url::from_file_path(&meta.buffile).unwrap(),
                },
                range,
                options: params.formatting_options.clone(),
                work_done_progress_params: Default::default(),
            })
            .collect(),
    );

    ctx.call::<RangeFormatting, _>(
        meta,
        RequestParams::Each(req_params),
        move |ctx, meta, results| {
            let text_edits = results
                .into_iter()
                .filter_map(|(_, v)| v)
                .flatten()
                .collect::<Vec<_>>();
            editor_range_formatting(meta, (server_id, text_edits), ctx)
        },
    );
}

pub fn editor_range_formatting<T: TextEditish<T>>(
    meta: EditorMeta,
    result: (ServerId, Vec<T>),
    ctx: &mut Context,
) {
    let (server_id, text_edits) = result;
    let server = ctx.server(server_id);
    let cmd = ctx.documents.get(&meta.buffile).and_then(|document| {
        apply_text_edits_to_buffer(
            &meta.session,
            &meta.client,
            None,
            text_edits,
            &document.text,
            server.offset_encoding,
            false,
        )
    });
    match cmd {
        Some(cmd) => ctx.exec(meta, cmd),
        // Nothing to do, but sending command back to the editor is required to handle case when
        // editor is blocked waiting for response via fifo.
        None => ctx.exec(meta, "nop"),
    }
}
