use crate::screen::dashboard::pane::{self, Message};
use crate::style::{self, Icon, icon_text};
use crate::widget::{column_drag, dragger_row, labeled_slider};

use crate::widget::pick_list;
use data::chart::indicator::{Indicator, KlineIndicator, UiIndicator};
use data::chart::kline::drawing::DrawingColor;
use data::chart::kline::{
    BarAnalysisConfig, BubbleColorMode, BubbleLabelMode, BubbleThresholdMode,
    Config as KlineConfig, CvdRenderStyle, CvdReset, SessionProfileInterval, SessionProfileMode,
    SessionProfilePlacement, SmcConfig, VolumeBubblePreset, VolumeBubbleSession,
};
use data::layout::pane::VisualConfig;
use iced::{
    Element, Length, padding,
    widget::{button, checkbox, column, container, pane_grid, row, space, text, text_input},
};

fn parse_optional_nonnegative(input: &str) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn format_optional_number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn drawing_color_to_hex(color: DrawingColor) -> String {
    data::config::theme::color_to_hex(iced::Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    })
}

fn parse_drawing_color(input: &str) -> Option<DrawingColor> {
    let color = data::config::theme::hex_to_color(input.trim())?;
    Some(DrawingColor {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    })
}

pub fn view<'a, I>(
    pane: pane_grid::Pane,
    state: &'a pane::State,
    selected: &[I],
    market_type: Option<exchange::adapter::MarketKind>,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let content_allows_dragging = matches!(state.content, pane::Content::Kline { .. });
    let content_row = if let Some(market) = market_type {
        content_row(
            pane,
            &state.content,
            selected,
            market,
            content_allows_dragging,
        )
    } else {
        column![].spacing(4).into()
    };

    container(content_row)
        .width(iced::Length::Fit.max(200))
        .padding(16)
        .style(style::chart_modal)
        .into()
}

pub fn view_kline<'a>(
    pane: pane_grid::Pane,
    state: &'a pane::State,
    selected: &[KlineIndicator],
    market_type: Option<exchange::adapter::MarketKind>,
    cfg: KlineConfig,
    _bubble_scale: crate::chart::kline::VolumeBubbleQtyScale,
) -> Element<'a, Message> {
    let list: Element<'a, Message> = if let Some(market) = market_type {
        content_row(pane, &state.content, selected, market, true)
    } else {
        column![].into()
    };
    let mut sections = column![list].spacing(12);

    let pine_directories = crate::chart::pine::external_indicator_directories();
    let pine_directory = pine_directories
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "indicator (next to the release executable)".to_string());
    let pine_summaries = crate::chart::pine::external_indicator_summaries();
    let pine_rows: Vec<Element<'a, Message>> = if pine_summaries.is_empty() {
        vec![text("No .pine, .pine5 or .pine6 files found").into()]
    } else {
        pine_summaries
            .into_iter()
            .map(|indicator| {
                let state = if indicator.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let status = indicator
                    .error
                    .map_or_else(|| "ready".to_string(), |error| format!("error: {error}"));
                column![
                    row![
                        text(indicator.name),
                        space::horizontal(),
                        text(format!("v{} · {state}", indicator.version))
                            .size(crate::style::text_size::SMALL),
                    ],
                    text(format!("{} · {status}", indicator.path.display()))
                        .size(crate::style::text_size::SMALL),
                ]
                .spacing(2)
                .into()
            })
            .collect()
    };
    let mut pine_list = iced::widget::Column::with_children(pine_rows).spacing(6);
    pine_list = pine_list.push(
        row![
            text(format!("Folder: {pine_directory}")).size(crate::style::text_size::SMALL),
            space::horizontal(),
            button("Refresh").on_press(Message::PaneEvent(pane, pane::Event::ReloadPineScripts,)),
        ]
        .spacing(8),
    );
    sections = sections.push(indicator_card(
        "External Pine indicators (v5 / v6)",
        column![
            text("Put files beside the release binary in indicator/; the legacy user folders are also scanned."),
            pine_list,
        ]
        .spacing(6),
    ));

    if selected.contains(&KlineIndicator::BarAnalysis) {
        let table = cfg.bar_analysis;
        let toggle =
            |label: &'static str, current: bool, change: fn(&mut BarAnalysisConfig, bool)| {
                checkbox(current).label(label).on_toggle(move |value| {
                    let mut next = table;
                    change(&mut next, value);
                    config_message(
                        pane,
                        KlineConfig {
                            bar_analysis: next,
                            ..cfg
                        },
                    )
                })
            };
        let min_delta = text_input("No minimum", format_optional_number(table.min_delta_filter))
            .on_input(move |value| {
                config_message(
                    pane,
                    KlineConfig {
                        bar_analysis: BarAnalysisConfig {
                            min_delta_filter: parse_optional_nonnegative(&value),
                            ..table
                        },
                        ..cfg
                    },
                )
            });
        let max_delta = text_input("No maximum", format_optional_number(table.max_delta_filter))
            .on_input(move |value| {
                config_message(
                    pane,
                    KlineConfig {
                        bar_analysis: BarAnalysisConfig {
                            max_delta_filter: parse_optional_nonnegative(&value),
                            ..table
                        },
                        ..cfg
                    },
                )
            });

        sections = sections.push(indicator_card(
            "Data Table / Bar Analysis",
            column![
                toggle("Buy", table.show_buy, |config, value| config.show_buy =
                    value),
                toggle("Sell", table.show_sell, |config, value| config.show_sell =
                    value),
                toggle("Total volume", table.show_volume, |config, value| config
                    .show_volume =
                    value),
                toggle("Delta", table.show_delta, |config, value| config
                    .show_delta =
                    value),
                toggle("Delta %", table.show_delta_pct, |config, value| config
                    .show_delta_pct =
                    value),
                toggle("Minimum delta", table.show_min_delta, |config, value| {
                    config.show_min_delta = value
                }),
                toggle("Maximum delta", table.show_max_delta, |config, value| {
                    config.show_max_delta = value
                }),
                row![text("Minimum delta filter"), min_delta].spacing(8),
                row![text("Maximum delta filter"), max_delta].spacing(8),
            ]
            .spacing(6),
        ));
    }

    if selected.contains(&KlineIndicator::CumulativeDelta) {
        let cvd = cfg.cvd;
        let render_style = pick_list(
            CvdRenderStyle::ALL,
            Some(cvd.render_style),
            move |render_style| {
                config_message(
                    pane,
                    KlineConfig {
                        cvd: data::chart::kline::CvdConfig {
                            render_style,
                            ..cvd
                        },
                        ..cfg
                    },
                )
            },
        );
        let candle_width = labeled_slider(
            "Candle width",
            10.0..=100.0,
            cvd.candle_width_percent,
            move |candle_width_percent| {
                config_message(
                    pane,
                    KlineConfig {
                        cvd: data::chart::kline::CvdConfig {
                            candle_width_percent,
                            ..cvd
                        },
                        ..cfg
                    },
                )
            },
            |value| format!("{value:.0}%"),
            Some(1.0),
        );
        let line_width = labeled_slider(
            "Line width",
            0.5..=5.0,
            cvd.line_width,
            move |line_width| {
                config_message(
                    pane,
                    KlineConfig {
                        cvd: data::chart::kline::CvdConfig { line_width, ..cvd },
                        ..cfg
                    },
                )
            },
            |value| format!("{value:.1}px"),
            Some(0.1),
        );
        let show_wicks =
            checkbox(cvd.show_wicks)
                .label("Show wicks")
                .on_toggle(move |show_wicks| {
                    config_message(
                        pane,
                        KlineConfig {
                            cvd: data::chart::kline::CvdConfig { show_wicks, ..cvd },
                            ..cfg
                        },
                    )
                });
        let reset = pick_list(CvdReset::ALL, Some(cvd.reset), move |reset| {
            config_message(
                pane,
                KlineConfig {
                    cvd: data::chart::kline::CvdConfig { reset, ..cvd },
                    ..cfg
                },
            )
        });
        let style_controls: Element<'a, Message> = match cvd.render_style {
            CvdRenderStyle::Candlesticks => column![candle_width, show_wicks].spacing(6).into(),
            CvdRenderStyle::Line => column![line_width].spacing(6).into(),
        };
        sections = sections.push(indicator_card(
            "Cumulative Volume Delta",
            column![render_style, reset, style_controls].spacing(6),
        ));
    }

    if selected.contains(&KlineIndicator::SessionVolumeProfile) {
        let svp = cfg.session_volume_profile;
        let interval =
            pick_list(
                SessionProfileInterval::ALL,
                Some(svp.interval),
                move |interval| {
                    config_message(
                        pane,
                        KlineConfig {
                            session_volume_profile:
                                data::chart::kline::SessionVolumeProfileConfig { interval, ..svp },
                            ..cfg
                        },
                    )
                },
            );
        let placement =
            pick_list(
                SessionProfilePlacement::ALL,
                Some(svp.placement),
                move |placement| {
                    config_message(
                        pane,
                        KlineConfig {
                            session_volume_profile:
                                data::chart::kline::SessionVolumeProfileConfig { placement, ..svp },
                            ..cfg
                        },
                    )
                },
            );
        let mode = pick_list(SessionProfileMode::ALL, Some(svp.mode), move |mode| {
            config_message(
                pane,
                KlineConfig {
                    session_volume_profile: data::chart::kline::SessionVolumeProfileConfig {
                        mode,
                        ..svp
                    },
                    ..cfg
                },
            )
        });
        let width = labeled_slider(
            "Width",
            10.0..=90.0,
            svp.width_percent,
            move |width_percent| {
                config_message(
                    pane,
                    KlineConfig {
                        session_volume_profile: data::chart::kline::SessionVolumeProfileConfig {
                            width_percent,
                            ..svp
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}%"),
            Some(1.0),
        );
        let value_area = labeled_slider(
            "Value area",
            50.0..=95.0,
            svp.value_area_percent,
            move |value_area_percent| {
                config_message(
                    pane,
                    KlineConfig {
                        session_volume_profile: data::chart::kline::SessionVolumeProfileConfig {
                            value_area_percent,
                            ..svp
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}%"),
            Some(1.0),
        );
        let rows = labeled_slider(
            "Ticks / row",
            1.0..=50.0,
            svp.row_size_ticks as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        session_volume_profile: data::chart::kline::SessionVolumeProfileConfig {
                            row_size_ticks: v as u16,
                            ..svp
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let poc =
            checkbox(svp.show_poc)
                .label("POC")
                .on_toggle(move |show_poc| {
                    config_message(
                        pane,
                        KlineConfig {
                            session_volume_profile:
                                data::chart::kline::SessionVolumeProfileConfig { show_poc, ..svp },
                            ..cfg
                        },
                    )
                });
        let va =
            checkbox(svp.show_value_area)
                .label("VAH / VAL")
                .on_toggle(move |show_value_area| {
                    config_message(
                        pane,
                        KlineConfig {
                            session_volume_profile:
                                data::chart::kline::SessionVolumeProfileConfig {
                                    show_value_area,
                                    ..svp
                                },
                            ..cfg
                        },
                    )
                });
        let vwap =
            checkbox(svp.show_vwap)
                .label("Session VWAP level")
                .on_toggle(move |show_vwap| {
                    config_message(
                        pane,
                        KlineConfig {
                            session_volume_profile:
                                data::chart::kline::SessionVolumeProfileConfig { show_vwap, ..svp },
                            ..cfg
                        },
                    )
                });
        let hi_lo = checkbox(svp.show_session_high_low)
            .label("Session high / low")
            .on_toggle(move |show_session_high_low| {
                config_message(
                    pane,
                    KlineConfig {
                        session_volume_profile: data::chart::kline::SessionVolumeProfileConfig {
                            show_session_high_low,
                            ..svp
                        },
                        ..cfg
                    },
                )
            });
        sections = sections.push(indicator_card(
            "Session Volume Profile",
            column![
                interval,
                placement,
                mode,
                width,
                value_area,
                rows,
                row![poc, va].spacing(8),
                vwap,
                hi_lo
            ]
            .spacing(6),
        ));
    }

    if selected.contains(&KlineIndicator::VolumeBubbles) {
        let bubbles = cfg.volume_bubbles;
        let preset = pick_list(
            VolumeBubblePreset::ALL,
            Some(bubbles.preset),
            move |preset| {
                let mut volume_bubbles = data::chart::kline::VolumeBubbleConfig::for_preset(preset);
                volume_bubbles.enabled = bubbles.enabled;
                volume_bubbles.three_dimensional = bubbles.three_dimensional;
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles,
                        ..cfg
                    },
                )
            },
        );
        let threshold_mode = pick_list(
            BubbleThresholdMode::ALL,
            Some(bubbles.threshold_mode),
            move |threshold_mode| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            threshold_mode,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            },
        );
        let session = pick_list(
            VolumeBubbleSession::ALL,
            Some(bubbles.session),
            move |session| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            session,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
        );
        let mode = pick_list(
            BubbleColorMode::ALL,
            Some(bubbles.color_mode),
            move |color_mode| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            color_mode,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
        );
        let count = labeled_slider(
            "Max / candle",
            1.0..=10.0,
            bubbles.max_bubbles_per_bar as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            max_bubbles_per_bar: v as usize,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let viewport_count = labeled_slider(
            "Max / viewport",
            5.0..=100.0,
            bubbles.max_bubbles_in_view as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            max_bubbles_in_view: v as usize,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let cluster_time = labeled_slider(
            "Cluster time",
            100.0..=1_500.0,
            bubbles.cluster_window_ms as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            cluster_window_ms: v as u32,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}ms"),
            Some(50.0),
        );
        let display_percentile = labeled_slider(
            "Display percentile",
            80.0..=99.9,
            bubbles.display_percentile,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            display_percentile: v,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.1}"),
            Some(0.5),
        );
        let age_fading =
            checkbox(bubbles.age_fading)
                .label("Age fading")
                .on_toggle(move |age_fading| {
                    config_message(
                        pane,
                        KlineConfig {
                            volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                                age_fading,
                                ..bubbles
                            }
                            .customized(),
                            ..cfg
                        },
                    )
                });
        let three_dimensional = checkbox(bubbles.three_dimensional)
            .label("3D bubbles")
            .on_toggle(move |three_dimensional| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            three_dimensional,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            });
        let price_response = checkbox(bubbles.price_response_enabled)
            .label("Price response analysis")
            .on_toggle(move |price_response_enabled| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            price_response_enabled,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            });
        let candidates = labeled_slider(
            "Historical candidates",
            1.0..=20.0,
            bubbles.max_candidates_per_candle as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            max_candidates_per_candle: v as usize,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let history = labeled_slider(
            "History window",
            1.0..=120.0,
            bubbles.history_window_minutes as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            history_window_minutes: v as u64,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}m"),
            Some(1.0),
        );
        let min_qty =
            text_input("Base-asset quantity", bubbles.min_qty.to_string()).on_input(move |value| {
                let min_qty = value
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .unwrap_or(bubbles.min_qty);
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            min_qty,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            });
        let min_radius = labeled_slider(
            "Minimum radius",
            1.0..=20.0,
            bubbles.min_radius_px,
            move |min_radius_px| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            min_radius_px,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}px"),
            Some(1.0),
        );
        let max_radius = labeled_slider(
            "Maximum radius",
            4.0..=40.0,
            bubbles.max_radius_px,
            move |max_radius_px| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            max_radius_px,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}px"),
            Some(1.0),
        );
        let labels = pick_list(
            BubbleLabelMode::ALL,
            Some(bubbles.label_mode),
            move |label_mode| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            label_mode,
                            ..bubbles
                        }
                        .customized(),
                        ..cfg
                    },
                )
            },
        );
        let reuse = checkbox(bubbles.use_raw_trades_when_available)
            .label("Reuse shared raw trades")
            .on_toggle(move |use_raw_trades_when_available| {
                config_message(
                    pane,
                    KlineConfig {
                        volume_bubbles: data::chart::kline::VolumeBubbleConfig {
                            use_raw_trades_when_available,
                            ..bubbles
                        },
                        ..cfg
                    },
                )
            });
        sections = sections.push(indicator_card(
            "Volume Bubbles",
            column![
                preset,
                session,
                mode,
                labels,
                threshold_mode,
                display_percentile,
                min_qty,
                cluster_time,
                count,
                viewport_count,
                min_radius,
                max_radius,
                three_dimensional,
                age_fading,
                price_response,
                history,
                candidates,
                reuse
            ]
            .spacing(6),
        ));
    }

    if selected.contains(&KlineIndicator::Vwap) {
        let vwap = cfg.vwap;
        let anchor = pick_list(
            SessionProfileInterval::ALL,
            Some(vwap.anchor),
            move |anchor| {
                config_message(
                    pane,
                    KlineConfig {
                        vwap: data::chart::kline::VwapConfig { anchor, ..vwap },
                        ..cfg
                    },
                )
            },
        );
        let width = labeled_slider(
            "Line width",
            0.5..=5.0,
            vwap.line_width,
            move |line_width| {
                config_message(
                    pane,
                    KlineConfig {
                        vwap: data::chart::kline::VwapConfig { line_width, ..vwap },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.1}px"),
            Some(0.1),
        );
        let bands = checkbox(vwap.show_bands)
            .label("Standard-deviation bands")
            .on_toggle(move |show_bands| {
                config_message(
                    pane,
                    KlineConfig {
                        vwap: data::chart::kline::VwapConfig { show_bands, ..vwap },
                        ..cfg
                    },
                )
            });
        let mut band_controls = column![].spacing(4);
        for (index, band) in vwap.bands.iter().copied().enumerate() {
            let enabled = checkbox(band.enabled)
                .label(format!("{}σ band", band.sigma))
                .on_toggle(move |enabled| {
                    let mut bands = vwap.bands;
                    bands[index].enabled = enabled;
                    config_message(
                        pane,
                        KlineConfig {
                            vwap: data::chart::kline::VwapConfig { bands, ..vwap },
                            ..cfg
                        },
                    )
                });
            let sigma = text_input("0.5", band.sigma.to_string()).on_input(move |value| {
                let sigma = parse_optional_nonnegative(&value)
                    .map(|value| value as f32)
                    .unwrap_or(band.sigma);
                let mut bands = vwap.bands;
                bands[index].sigma = sigma;
                config_message(
                    pane,
                    KlineConfig {
                        vwap: data::chart::kline::VwapConfig { bands, ..vwap },
                        ..cfg
                    },
                )
            });
            let color =
                text_input("#RRGGBB", drawing_color_to_hex(band.color)).on_input(move |value| {
                    let color = parse_drawing_color(&value).unwrap_or(band.color);
                    let mut bands = vwap.bands;
                    bands[index].color = color;
                    config_message(
                        pane,
                        KlineConfig {
                            vwap: data::chart::kline::VwapConfig { bands, ..vwap },
                            ..cfg
                        },
                    )
                });
            let background = checkbox(band.show_background)
                .label("Background")
                .on_toggle(move |show_background| {
                    let mut bands = vwap.bands;
                    bands[index].show_background = show_background;
                    config_message(
                        pane,
                        KlineConfig {
                            vwap: data::chart::kline::VwapConfig { bands, ..vwap },
                            ..cfg
                        },
                    )
                });
            let alpha = labeled_slider(
                "Background alpha",
                0.0..=0.5,
                band.background_alpha,
                move |background_alpha| {
                    let mut bands = vwap.bands;
                    bands[index].background_alpha = background_alpha;
                    config_message(
                        pane,
                        KlineConfig {
                            vwap: data::chart::kline::VwapConfig { bands, ..vwap },
                            ..cfg
                        },
                    )
                },
                |value| format!("{value:.2}"),
                Some(0.01),
            );
            band_controls = band_controls.push(
                column![
                    enabled,
                    row![text("Sigma"), sigma, text("Color"), color].spacing(6),
                    background,
                    alpha,
                ]
                .spacing(4),
            );
        }
        let labels = checkbox(vwap.show_labels)
            .label("Labels")
            .on_toggle(move |show_labels| {
                config_message(
                    pane,
                    KlineConfig {
                        vwap: data::chart::kline::VwapConfig {
                            show_labels,
                            ..vwap
                        },
                        ..cfg
                    },
                )
            });
        sections = sections.push(indicator_card(
            "VWAP",
            column![anchor, width, bands, labels, band_controls].spacing(6),
        ));
    }

    if selected.contains(&KlineIndicator::GexLevels) {
        use data::chart::gex::{GexExpiryFilter, GexGammaSource, GexLevelColor, GexLevelsConfig};
        let levels = cfg.gex_levels();
        let proxy_available = matches!(
            &state.content,
            pane::Content::Kline { chart: Some(chart), .. } if chart.gex_proxy_available()
        );
        let provider_status = if proxy_available {
            "24h proxy history: GEX Monitor, Live profile: Deribit"
        } else {
            "Live profile: Deribit, Local history only"
        };
        let update = move |next: GexLevelsConfig| config_message(pane, cfg.with_gex_levels(next));
        let toggle =
            |label: &'static str, current: bool, change: fn(&mut GexLevelsConfig, bool)| {
                checkbox(current).label(label).on_toggle(move |value| {
                    let mut next = levels;
                    change(&mut next, value);
                    update(next)
                })
            };
        let expiry = pick_list(
            GexExpiryFilter::ALL,
            Some(levels.expiry_filter),
            move |expiry_filter| {
                update(GexLevelsConfig {
                    expiry_filter,
                    ..levels
                })
            },
        );
        let gamma_source = pick_list(
            GexGammaSource::ALL,
            Some(levels.gamma_source),
            move |gamma_source| {
                update(GexLevelsConfig {
                    gamma_source,
                    ..levels
                })
            },
        );
        let positive_color = pick_list(
            GexLevelColor::ALL,
            Some(levels.positive_color),
            move |positive_color| {
                update(GexLevelsConfig {
                    positive_color,
                    ..levels
                })
            },
        );
        let negative_color = pick_list(
            GexLevelColor::ALL,
            Some(levels.negative_color),
            move |negative_color| {
                update(GexLevelsConfig {
                    negative_color,
                    ..levels
                })
            },
        );
        let minimum_oi = text_input("0 = disabled", levels.minimum_open_interest.to_string())
            .on_input(move |value| {
                let minimum_open_interest =
                    parse_optional_nonnegative(&value).unwrap_or(levels.minimum_open_interest);
                update(GexLevelsConfig {
                    minimum_open_interest,
                    ..levels
                })
            });
        let minimum_gex = text_input("0 = disabled", levels.minimum_absolute_gex.to_string())
            .on_input(move |value| {
                let minimum_absolute_gex =
                    parse_optional_nonnegative(&value).unwrap_or(levels.minimum_absolute_gex);
                update(GexLevelsConfig {
                    minimum_absolute_gex,
                    ..levels
                })
            });
        let minimum_strength = text_input("0.00 - 1.00", levels.minimum_zone_strength.to_string())
            .on_input(move |value| {
                let minimum_zone_strength = parse_optional_nonnegative(&value)
                    .map(|value| value.min(1.0) as f32)
                    .unwrap_or(levels.minimum_zone_strength);
                update(GexLevelsConfig {
                    minimum_zone_strength,
                    ..levels
                })
            });
        let level_match_tolerance = text_input(
            "0.00 - 1.00",
            levels.level_match_tolerance_percent.to_string(),
        )
        .on_input(move |value| {
            let level_match_tolerance_percent = parse_optional_nonnegative(&value)
                .map(|value| value.min(1.0) as f32)
                .unwrap_or(levels.level_match_tolerance_percent);
            update(GexLevelsConfig {
                level_match_tolerance_percent,
                ..levels
            })
        });
        let max_positive = labeled_slider(
            "Maximum positive zones",
            1.0..=6.0,
            f32::from(levels.max_positive_zones),
            move |value| {
                update(GexLevelsConfig {
                    max_positive_zones: value as u8,
                    ..levels
                })
            },
            |value| format!("{value:.0}"),
            Some(1.0),
        );
        let max_negative = labeled_slider(
            "Maximum negative zones",
            1.0..=6.0,
            f32::from(levels.max_negative_zones),
            move |value| {
                update(GexLevelsConfig {
                    max_negative_zones: value as u8,
                    ..levels
                })
            },
            |value| format!("{value:.0}"),
            Some(1.0),
        );
        let persistence = labeled_slider(
            "Persistence lookback",
            5.0..=60.0,
            f32::from(levels.persistent_lookback_minutes),
            move |value| {
                update(GexLevelsConfig {
                    persistent_lookback_minutes: value as u16,
                    ..levels
                })
            },
            |value| format!("{value:.0} min"),
            Some(1.0),
        );
        let fade = labeled_slider(
            "Fade buckets",
            1.0..=3.0,
            f32::from(levels.fade_buckets),
            move |value| {
                update(GexLevelsConfig {
                    fade_buckets: value as u8,
                    ..levels
                })
            },
            |value| format!("{value:.0}"),
            Some(1.0),
        );
        let profile_width = labeled_slider(
            "Profile width",
            4.0..=7.0,
            levels.current_profile_width_percent,
            move |current_profile_width_percent| {
                update(GexLevelsConfig {
                    current_profile_width_percent,
                    ..levels
                })
            },
            |value| format!("{value:.0}%"),
            Some(1.0),
        );
        let settings = column![
            text("Data").size(crate::style::text_size::SECTION),
            text("Expiry filter"),
            expiry,
            text("Gamma source"),
            gamma_source,
            row![text("Minimum OI"), minimum_oi].spacing(8),
            row![text("Minimum absolute GEX"), minimum_gex].spacing(8),
            text("Zones").size(crate::style::text_size::SECTION),
            row![text("Positive"), positive_color].spacing(8),
            row![text("Negative"), negative_color].spacing(8),
            row![text("Minimum zone strength"), minimum_strength].spacing(8),
            row![text("Level match tolerance"), level_match_tolerance].spacing(8),
            max_positive,
            max_negative,
            persistence,
            fade,
            toggle(
                "Show historical zones",
                levels.show_historical_zones,
                |c, v| c.show_historical_zones = v
            ),
            toggle(
                "Show active projection",
                levels.show_active_projection,
                |c, v| c.show_active_projection = v
            ),
            text("Profile").size(crate::style::text_size::SECTION),
            toggle(
                "Show current profile",
                levels.show_current_profile,
                |c, v| c.show_current_profile = v
            ),
            profile_width,
            text("Levels").size(crate::style::text_size::SECTION),
            toggle("Show CW", levels.show_call_wall_marker, |c, v| c
                .show_call_wall_marker =
                v),
            toggle("Show PW", levels.show_put_wall_marker, |c, v| c
                .show_put_wall_marker =
                v),
            toggle("Show GF", levels.show_gamma_flip_marker, |c, v| c
                .show_gamma_flip_marker =
                v),
            text("Interaction").size(crate::style::text_size::SECTION),
            toggle("Show tooltip", levels.show_hover_tooltip, |c, v| c
                .show_hover_tooltip =
                v),
            text(provider_status).size(crate::style::text_size::SMALL),
        ]
        .spacing(6);
        sections = sections.push(indicator_card("GEX Overlay", settings));
    }

    if selected.contains(&KlineIndicator::SmartMoney) {
        let smc = cfg.smc;
        let swing_len = labeled_slider(
            "Swing length",
            10.0..=200.0,
            smc.swing_length as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        smc: SmcConfig {
                            swing_length: v as i32,
                            ..smc
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let ob_count = labeled_slider(
            "Order blocks",
            1.0..=20.0,
            smc.order_blocks_count as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        smc: SmcConfig {
                            order_blocks_count: v as i32,
                            ..smc
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let eq_length = labeled_slider(
            "EQ bars confirmation",
            1.0..=10.0,
            smc.equal_highs_lows_length as f32,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        smc: SmcConfig {
                            equal_highs_lows_length: v as i32,
                            ..smc
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.0}"),
            Some(1.0),
        );
        let eq_threshold = labeled_slider(
            "EQ threshold",
            0.0..=0.5,
            smc.equal_highs_lows_threshold,
            move |v| {
                config_message(
                    pane,
                    KlineConfig {
                        smc: SmcConfig {
                            equal_highs_lows_threshold: v,
                            ..smc
                        },
                        ..cfg
                    },
                )
            },
            |v| format!("{v:.2}"),
            Some(0.01),
        );
        let settings = column![
            checkbox(smc.show_swing_structure)
                .label("Swing structure")
                .on_toggle(move |show_swing_structure| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_swing_structure,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            checkbox(smc.show_swing_order_blocks)
                .label("Swing order blocks")
                .on_toggle(move |show_swing_order_blocks| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_swing_order_blocks,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            checkbox(smc.show_equal_highs_lows)
                .label("Equal highs/lows")
                .on_toggle(move |show_equal_highs_lows| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_equal_highs_lows,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            checkbox(smc.show_fair_value_gaps)
                .label("Fair value gaps")
                .on_toggle(move |show_fair_value_gaps| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_fair_value_gaps,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            checkbox(smc.show_premium_discount_zones)
                .label("Premium/discount zones")
                .on_toggle(move |show_premium_discount_zones| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_premium_discount_zones,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            checkbox(smc.show_strong_weak_high_low)
                .label("Strong/weak high/low")
                .on_toggle(move |show_strong_weak_high_low| {
                    config_message(
                        pane,
                        KlineConfig {
                            smc: SmcConfig {
                                show_strong_weak_high_low,
                                ..smc
                            },
                            ..cfg
                        },
                    )
                }),
            swing_len,
            ob_count,
            eq_length,
            eq_threshold,
        ]
        .spacing(6);
        sections = sections.push(indicator_card("Smart Money Concepts", settings));
    }

    container(crate::widget::scrollable_content(sections))
        .width(iced::Length::Fit.max(340))
        .padding(16)
        .style(style::chart_modal)
        .into()
}

fn config_message(pane: pane_grid::Pane, cfg: KlineConfig) -> Message {
    Message::VisualConfigChanged(pane, VisualConfig::Kline(cfg), false)
}

fn indicator_card<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let content: Element<'a, Message> = content.into();
    container(column![text(title).size(crate::style::text_size::SECTION), content].spacing(8))
        .padding(10)
        .style(style::chart_modal)
        .into()
}

fn build_indicator_row<'a, I>(
    pane: pane_grid::Pane,
    indicator: &I,
    is_selected: bool,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let content = if is_selected {
        row![
            text(indicator.to_string()),
            space::horizontal(),
            container(icon_text(Icon::Checkmark, 12)),
        ]
        .width(Length::Fill)
    } else {
        row![text(indicator.to_string())].width(Length::Fill)
    };

    button(content)
        .on_press(Message::PaneEvent(
            pane,
            pane::Event::ToggleIndicator((*indicator).into()),
        ))
        .width(Length::Fill)
        .style(move |theme, status| style::button::modifier(theme, status, is_selected))
        .into()
}

fn selected_list<'a, I>(
    pane: pane_grid::Pane,
    selected: &[I],
    reorderable: bool,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let elements: Vec<Element<_>> = selected
        .iter()
        .map(|indicator| {
            let base = build_indicator_row(pane, indicator, true);
            dragger_row(base, reorderable)
        })
        .collect();

    if reorderable {
        let mut draggable_column = column_drag::Column::new()
            .on_drag(move |event| Message::PaneEvent(pane, pane::Event::ReorderIndicator(event)))
            .spacing(4);
        for element in elements {
            draggable_column = draggable_column.push(element);
        }
        draggable_column.into()
    } else {
        iced::widget::Column::with_children(elements)
            .spacing(4)
            .into()
    }
}

fn indicator_toggle_states<I: Copy + PartialEq>(
    indicators: &[I],
    selected: &[I],
) -> Vec<(I, bool)> {
    indicators
        .iter()
        .copied()
        .map(|indicator| (indicator, selected.contains(&indicator)))
        .collect()
}

fn indicator_toggle_list<'a, I>(
    pane: pane_grid::Pane,
    indicators: &[(I, bool)],
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let elements: Vec<Element<_>> = indicators
        .iter()
        .map(|(indicator, is_selected)| {
            let base = build_indicator_row(pane, indicator, *is_selected);
            dragger_row(base, false)
        })
        .collect();

    iced::widget::Column::with_children(elements)
        .spacing(4)
        .into()
}

fn content_row<'a, I>(
    pane: pane_grid::Pane,
    content: &pane::Content,
    selected: &[I],
    market: exchange::adapter::MarketKind,
    allows_drag: bool,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let reorderable = allows_drag && selected.len() >= 2;

    let selected: Vec<I> = selected
        .iter()
        .copied()
        .filter(|indicator| content.allows_indicator((*indicator).into()))
        .collect();

    // Keep every toggle in the market-defined order. Previously, enabling an indicator moved its
    // button from the available list to the selected list, making it appear to disappear when the
    // modal was scrolled.
    let indicators: Vec<I> = I::for_market(market)
        .iter()
        .copied()
        .filter(|indicator| content.allows_indicator((*indicator).into()))
        .collect();
    let toggles = indicator_toggle_states(&indicators, &selected);

    let mut col = iced::widget::Column::new()
        .push(indicator_toggle_list(pane, &toggles))
        .spacing(4);

    // Preserve drag reordering without using the active list as the toggle list.
    if reorderable {
        col = col
            .push(
                container(text("Active order").size(crate::style::text_size::SECTION))
                    .padding(padding::top(8).bottom(4)),
            )
            .push(selected_list(pane, &selected, true));
    }

    column![
        container(text("Indicators").size(crate::style::text_size::SECTION))
            .padding(padding::bottom(8)),
        col.spacing(4)
    ]
    .spacing(4)
    .into()
}

#[cfg(test)]
mod tests {
    use super::indicator_toggle_states;

    #[test]
    fn activating_indicator_keeps_toggle_order_and_marks_it_selected() {
        let indicators = ["Volume", "CVD", "VWAP"];

        assert_eq!(
            indicator_toggle_states(&indicators, &["CVD"]),
            vec![("Volume", false), ("CVD", true), ("VWAP", false)]
        );
        assert_eq!(
            indicator_toggle_states(&indicators, &["Volume", "CVD"]),
            vec![("Volume", true), ("CVD", true), ("VWAP", false)]
        );
    }
}
