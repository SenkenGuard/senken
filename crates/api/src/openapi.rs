//! The `OpenAPI` document.
//! 006 S1 for workspaces/bars/indicators/alerts).
//!
//! `utoipa` derives this from the same `serde` structs and handler
//! signatures the router actually uses (`crate::dto` plus every handler
//! module), so a field renamed in Rust breaks `openapi-typescript`'s
//! generated TypeScript rather than failing silently at runtime — the
//! property required here, and exactly what the web client needs to
//! regenerate the browser's types against this stage's new surface.
//! Served at `GET /api/openapi.json` (`EndpointPermission::Public`: the
//! document itself carries no account data) for `openapi-typescript` to
//! read from a running server.

use utoipa::OpenApi;

use crate::dto::{
    AccountAccessDto, ActionOutcomeDto, AdapterDto, AdaptersResponse, AddWatchlistMemberRequest,
    AlertDto, AlertsPage, AmendOrderRequest, AssetBalanceDto, AssignRoleRequest, BalancesDto,
    BarDto, BarJobDto, BarRangeResponse, BarsRequirementDto, BookCapabilityDto, CloseRequest,
    ComputeIndicatorRequest, ComputeIndicatorResponse, ConditionDto, CreateAlertRequest,
    CreateNoteRequest, CreateRoleRequest, CreateTradeAccountRequest, CreateUserRequest,
    CreateWatchlistGroupRequest, CreateWorkspaceRequest, DefaultWorkspaceResponse,
    DeleteStorageRequest, DeleteStorageResponse, DownloadM1Request, DrawingDto, DrawingInputDto,
    DrawingKindDto, DrawingLineStyleDto, DrawingPointDto, EnsureBarsRequest, EnsureBarsResponse,
    ErrorBody, FillDto, GrantDto, HealthDto, IdResponse, IndicatorCatalogEntry,
    IndicatorDrawableDto, IndicatorDrawablePointDto, IndicatorExtendDto, IndicatorLabelAnchorDto,
    IndicatorParamDefaultDto, IndicatorParamDto, IndicatorPlacementDto, IndicatorPlotDto,
    IndicatorPointDto, IndicatorPriceCoordDto, IndicatorScaleDto, IndicatorScaledPriceDto,
    IndicatorSpecDto, InstrumentSummaryDto, InstrumentsPage, LayerDto, LayerInputDto, LayerKindDto,
    LayoutDetailDto, LayoutSummaryDto, LoginRequest, LoginResponse, MarketDataUsageDto, MeResponse,
    NoteDto, NoteSummaryDto, NotesPage, OrderDto, PaneDto, PaneInputDto, PlaceOrderRequest,
    PluginGrantRequest, PositionDto, ProvisionalBarDto, RenameWatchlistGroupRequest,
    RenameWorkspaceRequest, ReorderWatchlistGroupsRequest, ReorderWatchlistMembersRequest,
    ReplaceLayoutRequest, ReplaceSettingsRequest, RoleSummaryDto, RolesPage, RunActionRequest,
    ScaledDto, SetPasswordRequest, SourceCapabilityDto, SourcesResponse, StorageDatabaseDto,
    StorageInstrumentDto, StorageReportDto, StorageSeriesDto, StorageSeriesKindDto,
    StorageSourceDto, TimeRangeDto, TradeAccountDto, TradeAccountSettingsDto, TradeAccountStateDto,
    TradeAccountsPage, UpdateNoteRequest, UpdateTradeAccountRequest,
    UpdateWorkspaceSettingsRequest, UserSummaryDto, UsersPage, WatchlistGroupDto,
    WatchlistGroupsPage, WatchlistMemberDto, WireInt, WorkspaceDto, WorkspacesPage,
    WsTicketResponse,
};
use crate::{
    Health, admin_handlers, alert_handlers, bars_handlers, identity_handlers, indicator_handlers,
    instrument_handlers, notes_handlers, source_handlers, storage_handlers, trade_handlers,
    watchlist_handlers, workspace_handlers, ws,
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Senken API",
        description = "Authentication, access control, workspaces, market data, indicators and alerts."
    ),
    paths(
        crate::health,
        identity_handlers::login,
        identity_handlers::logout,
        identity_handlers::set_password,
        identity_handlers::me,
        ws::issue_ticket,
        admin_handlers::list_users,
        admin_handlers::create_user,
        admin_handlers::list_roles,
        admin_handlers::create_role,
        admin_handlers::assign_role,
        admin_handlers::grant_direct,
        admin_handlers::revoke_direct,
        admin_handlers::grant_plugin_permission_to_user,
        admin_handlers::revoke_plugin_permission_from_user,
        admin_handlers::grant_plugin_permission_to_role,
        admin_handlers::revoke_plugin_permission_from_role,
        workspace_handlers::list_workspaces,
        workspace_handlers::create_workspace,
        workspace_handlers::default_workspace,
        workspace_handlers::rename_workspace,
        workspace_handlers::delete_workspace,
        workspace_handlers::update_workspace_settings,
        workspace_handlers::list_layouts,
        workspace_handlers::get_layout,
        workspace_handlers::replace_layout,
        workspace_handlers::update_layer,
        workspace_handlers::delete_layer,
        workspace_handlers::update_drawing,
        workspace_handlers::delete_drawing,
        bars_handlers::plan_bars,
        bars_handlers::range_bars,
        bars_handlers::ensure_bars,
        bars_handlers::download_m1,
        bars_handlers::bar_job_status,
        indicator_handlers::list_indicators,
        indicator_handlers::compute_indicator,
        alert_handlers::list_alerts,
        alert_handlers::get_alert,
        alert_handlers::create_alert,
        alert_handlers::delete_alert,
        instrument_handlers::search_instruments,
        source_handlers::list_sources,
        watchlist_handlers::list_watchlist_groups,
        watchlist_handlers::create_watchlist_group,
        watchlist_handlers::rename_watchlist_group,
        watchlist_handlers::delete_watchlist_group,
        watchlist_handlers::reorder_watchlist_groups,
        watchlist_handlers::list_watchlist_members,
        watchlist_handlers::add_watchlist_member,
        watchlist_handlers::remove_watchlist_member,
        watchlist_handlers::reorder_watchlist_members,
        notes_handlers::list_notes,
        notes_handlers::create_note,
        notes_handlers::get_note,
        notes_handlers::update_note,
        notes_handlers::delete_note,
        storage_handlers::storage_report,
        storage_handlers::delete_storage,
        trade_handlers::list_adapters,
        trade_handlers::list_accounts,
        trade_handlers::create_account,
        trade_handlers::account_state,
        trade_handlers::update_account,
        trade_handlers::delete_account,
        trade_handlers::get_settings,
        trade_handlers::replace_settings,
        trade_handlers::account_health,
        trade_handlers::account_balances,
        trade_handlers::account_positions,
        trade_handlers::account_orders,
        trade_handlers::account_fills,
        trade_handlers::place_order,
        trade_handlers::cancel_order,
        trade_handlers::amend_order,
        trade_handlers::close_position,
        trade_handlers::run_action,
    ),
    components(schemas(
        Health,
        ScaledDto,
        WireInt,
        AdapterDto,
        AdaptersResponse,
        TradeAccountDto,
        TradeAccountsPage,
        CreateTradeAccountRequest,
        AccountAccessDto,
        TradeAccountStateDto,
        UpdateTradeAccountRequest,
        TradeAccountSettingsDto,
        ReplaceSettingsRequest,
        AssetBalanceDto,
        BalancesDto,
        PositionDto,
        OrderDto,
        FillDto,
        PlaceOrderRequest,
        AmendOrderRequest,
        CloseRequest,
        HealthDto,
        RunActionRequest,
        ActionOutcomeDto,
        LoginRequest,
        LoginResponse,
        SetPasswordRequest,
        MeResponse,
        WsTicketResponse,
        ErrorBody,
        GrantDto,
        IdResponse,
        UserSummaryDto,
        UsersPage,
        RoleSummaryDto,
        RolesPage,
        CreateUserRequest,
        CreateRoleRequest,
        AssignRoleRequest,
        PluginGrantRequest,
        WorkspaceDto,
        WorkspacesPage,
        CreateWorkspaceRequest,
        RenameWorkspaceRequest,
        UpdateWorkspaceSettingsRequest,
        DefaultWorkspaceResponse,
        LayoutSummaryDto,
        LayoutDetailDto,
        PaneDto,
        PaneInputDto,
        LayerDto,
        LayerInputDto,
        LayerKindDto,
        DrawingDto,
        DrawingInputDto,
        DrawingKindDto,
        DrawingPointDto,
        DrawingLineStyleDto,
        ReplaceLayoutRequest,
        TimeRangeDto,
        BarsRequirementDto,
        BarDto,
        BarRangeResponse,
        EnsureBarsRequest,
        DownloadM1Request,
        EnsureBarsResponse,
        BarJobDto,
        IndicatorCatalogEntry,
        IndicatorParamDto,
        IndicatorParamDefaultDto,
        IndicatorPlotDto,
        IndicatorScaleDto,
        IndicatorPlacementDto,
        IndicatorSpecDto,
        ComputeIndicatorRequest,
        IndicatorDrawableDto,
        IndicatorDrawablePointDto,
        IndicatorPointDto,
        IndicatorExtendDto,
        IndicatorLabelAnchorDto,
        IndicatorScaledPriceDto,
        IndicatorPriceCoordDto,
        ComputeIndicatorResponse,
        ConditionDto,
        AlertDto,
        AlertsPage,
        CreateAlertRequest,
        InstrumentSummaryDto,
        ProvisionalBarDto,
        SourceCapabilityDto,
        BookCapabilityDto,
        SourcesResponse,
        InstrumentsPage,
        WatchlistGroupDto,
        WatchlistGroupsPage,
        CreateWatchlistGroupRequest,
        RenameWatchlistGroupRequest,
        ReorderWatchlistGroupsRequest,
        WatchlistMemberDto,
        AddWatchlistMemberRequest,
        ReorderWatchlistMembersRequest,
        NoteSummaryDto,
        NotesPage,
        NoteDto,
        CreateNoteRequest,
        UpdateNoteRequest,
        StorageReportDto,
        MarketDataUsageDto,
        StorageSourceDto,
        StorageInstrumentDto,
        StorageSeriesDto,
        StorageSeriesKindDto,
        StorageDatabaseDto,
        DeleteStorageRequest,
        DeleteStorageResponse,
    ))
)]
pub(crate) struct ApiDoc;

/// `GET /api/openapi.json`.
pub(crate) async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::ApiDoc;
    use utoipa::OpenApi;

    /// Writes the document to `SENKEN_OPENAPI_OUT` when that variable is
    /// set, so `openapi-typescript` can regenerate the browser's types
    /// without a server to point at.
    ///
    /// Silent otherwise, so an ordinary `cargo test` neither writes a file
    /// nor needs a network port. This is a maintenance tool that lives in
    /// the test binary because that is the only place `ApiDoc` — a private
    /// item — can be reached from.
    #[test]
    fn the_document_serialises_and_can_be_dumped_for_type_generation() {
        let json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap();
        assert!(json.contains("/api/trade/adapters"));
        if let Ok(path) = std::env::var("SENKEN_OPENAPI_OUT") {
            std::fs::write(path, json).unwrap();
        }
    }
}
