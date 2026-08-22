use crate::{I18N, app::graphs_window::plot::PlotColorScheme};
use gtk::{
    glib::object::Cast as _,
    prelude::{
        BoxExt, CheckButtonExt, DrawingAreaExtManual, GtkWindowExt, OrientableExt, WidgetExt,
    },
};
use i18n_embed_fl::fl;
use lact_schema::{ClocksTable, NvidiaDomainVfCurve};
use plotters::{
    chart::{ChartBuilder, SeriesLabelPosition},
    prelude::{Circle, EmptyElement, IntoDrawingArea as _, Rectangle, Text},
    series::{LineSeries, PointSeries},
    style::{Color as _, RGBColor, ShapeStyle, TextStyle, text_anchor::Pos},
};
use plotters_cairo::CairoBackend;
use relm4::{ComponentParts, RelmWidgetExt as _};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

/// In percentage, matching the GPC editor so hovering feels the same.
const POINT_VOLTAGE_HOVER_MARGIN: f32 = 0.01;
const POINT_FREQ_HOVER_MARGIN: f32 = 0.03;
const FREQ_RANGE_PADDING: u32 = 200;

/// How many curves the window can show at once.
const MAX_CURVES: usize = 4;

/// Fixed rather than taken from the theme, which only carries semantic colours
/// meant for backgrounds and washes these lines out against the plot.
const DOMAIN_COLORS: [RGBColor; MAX_CURVES] = [
    RGBColor(53, 132, 228),
    RGBColor(255, 120, 0),
    RGBColor(145, 65, 172),
    RGBColor(38, 162, 105),
];

/// The V/F curves of the clock domains fed by MSVDD.
///
/// A window of its own rather than another series on the GPC chart: those curves
/// are of a different rail, and sharing an axis would invite reading a
/// relationship between the two that does not exist, even though both happen to
/// use the same voltage grid.
#[derive(Clone)]
pub struct MsvddCurveEditor {
    curves: Rc<RefCell<Vec<NvidiaDomainVfCurve>>>,
    /// Which curves to draw, by position. Every curve starts shown.
    shown: Rc<RefCell<[bool; MAX_CURVES]>>,
    cursor_position: Rc<Cell<Option<(f64, f64)>>>,
    hovered_point: Rc<Cell<Option<(usize, usize)>>>,
}

#[derive(Debug)]
pub enum MsvddCurveEditorMsg {
    Show,
    Clocks(Option<Arc<ClocksTable>>),
    CursorUpdate { x: f64, y: f64 },
    ToggleCurve { index: usize, shown: bool },
}

#[relm4::component(pub)]
impl relm4::Component for MsvddCurveEditor {
    type Init = ();
    type Input = MsvddCurveEditorMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        adw::Window {
            set_hide_on_close: true,
            set_default_size: (1100, 700),
            set_title: Some(&fl!(I18N, "msvdd-curve-editor")),

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {},

                #[wrap(Some)]
                set_content = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_margin_all: 5,
                    set_spacing: 10,

                    gtk::Label {
                        set_markup: &fl!(I18N, "msvdd-curve-description"),
                        set_wrap: true,
                        set_max_width_chars: 90,
                        set_halign: gtk::Align::Center,
                    },

                    #[name = "toggles"]
                    gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_halign: gtk::Align::Center,
                        set_spacing: 15,
                    },

                    #[name = "drawing_area"]
                    gtk::DrawingArea {
                        set_expand: true,
                        set_margin_all: 10,

                        set_draw_func[model] => move |_, ctx, width, height| {
                            model.draw(ctx, width, height, PlotColorScheme::current());
                        },

                        add_controller = gtk::EventControllerMotion {
                            connect_motion[sender] => move |_, x, y| {
                                sender.input(MsvddCurveEditorMsg::CursorUpdate { x, y });
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: relm4::ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Self {
            curves: Rc::default(),
            shown: Rc::new(RefCell::new([true; MAX_CURVES])),
            cursor_position: Rc::new(Cell::new(None)),
            hovered_point: Rc::new(Cell::new(None)),
        };

        let widgets = view_output!();

        // One checkbox per slot, labelled and shown once the curves are known.
        for index in 0..MAX_CURVES {
            let button = gtk::CheckButton::builder().active(true).build();
            let sender = sender.clone();
            button.connect_toggled(move |button| {
                sender.input(MsvddCurveEditorMsg::ToggleCurve {
                    index,
                    shown: button.is_active(),
                });
            });
            widgets.toggles.append(&button);
        }

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        _sender: relm4::ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            MsvddCurveEditorMsg::Show => root.present(),
            MsvddCurveEditorMsg::Clocks(clocks_table) => {
                let mut curves = self.curves.borrow_mut();
                curves.clear();
                if let Some(ClocksTable::Nvidia(nvidia_table)) = clocks_table.as_deref() {
                    curves.clone_from(&nvidia_table.domain_vf_curves);
                }

                // Relabel the checkboxes for whatever the card reports, and hide
                // the ones with no curve behind them.
                let mut child = widgets.toggles.first_child();
                for index in 0..MAX_CURVES {
                    let Some(button) = child else { break };
                    child = button.next_sibling();

                    match curves.get(index) {
                        Some(curve) => {
                            button.set_visible(true);
                            if let Some(button) = button.downcast_ref::<gtk::CheckButton>() {
                                button.set_label(Some(&curve.name));
                            }
                        }
                        None => button.set_visible(false),
                    }
                }
            }
            MsvddCurveEditorMsg::CursorUpdate { x, y } => {
                self.cursor_position.set(Some((x, y)));
            }
            MsvddCurveEditorMsg::ToggleCurve { index, shown } => {
                if let Some(slot) = self.shown.borrow_mut().get_mut(index) {
                    *slot = shown;
                }
                // A curve that just disappeared must not stay hovered.
                self.hovered_point.set(None);
            }
        }

        widgets.drawing_area.queue_draw();
    }
}

impl MsvddCurveEditor {
    /// The curves that are both reported and switched on, with their colour.
    fn visible(&self) -> Vec<(usize, NvidiaDomainVfCurve, RGBColor)> {
        let shown = *self.shown.borrow();
        self.curves
            .borrow()
            .iter()
            .zip(DOMAIN_COLORS)
            .enumerate()
            .filter(|(index, _)| shown.get(*index).copied().unwrap_or(true))
            .map(|(index, (curve, color))| (index, curve.clone(), color))
            .collect()
    }

    fn draw(&self, ctx: &cairo::Context, width: i32, height: i32, colors: PlotColorScheme) {
        let backend = CairoBackend::new(ctx, (width as u32, height as u32)).unwrap();
        let root = backend.into_drawing_area();
        root.fill(&colors.background).unwrap();

        let visible = self.visible();
        if visible.is_empty() {
            return;
        }

        let all = || visible.iter().flat_map(|(_, curve, _)| curve.points.iter());
        let (Some(min_voltage), Some(max_voltage), Some(min_freq), Some(max_freq)) = (
            all().map(|point| point.voltage).min(),
            all().map(|point| point.voltage).max(),
            all().map(|point| point.freq).min(),
            all().map(|point| point.freq).max(),
        ) else {
            return;
        };

        let x_spec = min_voltage..max_voltage;
        let y_spec = min_freq.saturating_sub(FREQ_RANGE_PADDING)
            ..max_freq.saturating_add(FREQ_RANGE_PADDING);

        let mut chart = ChartBuilder::on(&root)
            .x_label_area_size(45)
            .y_label_area_size(110)
            .margin(50)
            .margin_bottom(20)
            .build_cartesian_2d(x_spec.clone(), y_spec.clone())
            .unwrap();

        chart
            .configure_mesh()
            .axis_style(colors.border_secondary)
            .bold_line_style(colors.border)
            .x_label_formatter(&|voltage| format!("{voltage} mV"))
            .y_label_formatter(&|clock| format!("{clock} MHz"))
            .x_label_style(("sans-serif", 14, &colors.text))
            .y_label_style(("sans-serif", 14, &colors.text))
            .x_desc(fl!(I18N, "msvdd-voltage"))
            .y_desc(fl!(I18N, "frequency"))
            .draw()
            .unwrap();

        let hovered = self.hovered_point.get();
        for (index, curve, color) in &visible {
            let series: Vec<(u32, u32)> = curve
                .points
                .iter()
                .map(|point| (point.voltage, point.freq))
                .collect();
            let style = color.mix(0.9);

            chart
                .draw_series(LineSeries::new(series.clone(), &style))
                .unwrap()
                .label(curve.name.clone())
                .legend(move |(x, y)| {
                    Rectangle::new([(x - 15, y + 2), (x, y - 1)], style.filled())
                });

            // Hollow, so they do not read as the draggable handles GPC has: these
            // curves are shown and never edited.
            let marker = ShapeStyle {
                color: style.to_rgba(),
                filled: false,
                stroke_width: 1,
            };
            let hovered = hovered
                .filter(|(curve_index, _)| curve_index == index)
                .map(|(_, point_index)| point_index);
            chart
                .draw_series(PointSeries::of_element(
                    series.into_iter().enumerate(),
                    2,
                    marker,
                    &|(point_index, coord), mut size, mut style| {
                        let text = if hovered == Some(point_index) {
                            style.filled = true;
                            size *= 3;
                            format!("{} MHz @ {} mV", coord.1, coord.0)
                        } else {
                            String::new()
                        };
                        let text_style = TextStyle {
                            font: ("sans-serif", 15).into(),
                            color: colors.text.to_backend_color(),
                            pos: Pos::default(),
                        };
                        EmptyElement::at(coord)
                            + Circle::new((0, 0), size, style)
                            + Text::new(text, (-170, -15), text_style)
                    },
                ))
                .unwrap();
        }

        chart
            .configure_series_labels()
            .position(SeriesLabelPosition::UpperLeft)
            .margin(20)
            .legend_area_size(5)
            .label_font(("sans-serif", 16, &colors.text))
            .background_style(colors.background.mix(0.6))
            .draw()
            .unwrap();

        // One frame behind, like the GPC chart: the hover used above is what the
        // previous draw worked out, and this is where the next one is decided.
        let translate = chart.into_coord_trans();
        let voltage_margin =
            (((max_voltage - min_voltage) as f32 * POINT_VOLTAGE_HOVER_MARGIN) as i32).max(1);
        let freq_margin =
            (((y_spec.end - y_spec.start) as f32 * POINT_FREQ_HOVER_MARGIN) as i32).max(1);

        let hovered = self
            .cursor_position
            .get()
            .and_then(|(x, y)| translate((x as i32, y as i32)))
            .and_then(|(voltage, freq)| {
                visible
                    .iter()
                    .flat_map(|(index, curve, _)| {
                        curve.points.iter().enumerate().map(move |(point, coords)| {
                            (
                                *index,
                                point,
                                (voltage as i32 - coords.voltage as i32).abs(),
                                (freq as i32 - coords.freq as i32).abs(),
                            )
                        })
                    })
                    .filter(|(_, _, voltage_distance, freq_distance)| {
                        *voltage_distance < voltage_margin && *freq_distance < freq_margin
                    })
                    // Both axes together, scaled to their own margins. Several
                    // curves have a point at any given voltage, and two of them run
                    // close enough that whichever is nearest horizontally is not the
                    // one under the cursor.
                    .min_by_key(|(_, _, voltage_distance, freq_distance)| {
                        let voltage =
                            i64::from(*voltage_distance) * 1000 / i64::from(voltage_margin);
                        let freq = i64::from(*freq_distance) * 1000 / i64::from(freq_margin);
                        voltage * voltage + freq * freq
                    })
                    .map(|(curve_index, point_index, _, _)| (curve_index, point_index))
            });
        self.hovered_point.set(hovered);
    }
}
