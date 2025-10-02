use krilla::tagging as kt;
use krilla::tagging::{LineHeight, NaiveRgbColor, Node, Tag, TextDecorationType};
use typst_library::diag::{SourceResult, bail};
use typst_library::foundations::{Content, Smart};
use typst_library::introspection::Location;
use typst_library::layout::{Abs, Length};
use typst_library::text::{Font, ScriptKind, TextItem, TextSize};
use typst_library::visualize::{Paint, Stroke};

use crate::PdfOptions;
use crate::tags::convert;
use crate::util::AbsExt;

#[derive(Clone, Debug)]
pub struct TextAttrs {
    /// Store the last resolved set of text attribute. The resolution isn't that
    /// expensive, but for large bodies of text it is resolved quite often.
    last_resolved: Option<(TextParams, ResolvedTextAttrs)>,
    items: Vec<(Location, TextAttr)>,
}

impl TextAttrs {
    pub const fn new() -> Self {
        Self { last_resolved: None, items: Vec::new() }
    }

    pub fn push_script(
        &mut self,
        elem: &Content,
        kind: ScriptKind,
        baseline_shift: Smart<Length>,
        lineheight: Smart<TextSize>,
    ) {
        let val = Script { kind, baseline_shift, lineheight };
        self.push(elem, TextAttr::Script(val));
    }

    pub fn push_highlight(&mut self, elem: &Content, paint: Option<&Paint>) {
        let color = paint.and_then(convert::paint_to_color);
        self.push(elem, TextAttr::Highlight(color));
    }

    pub fn push_deco(
        &mut self,
        options: &PdfOptions,
        elem: &Content,
        kind: TextDecoKind,
        stroke: Smart<Stroke>,
    ) -> SourceResult<()> {
        let stroke = TextDecoStroke::from(stroke);
        let deco = TextDeco { kind, stroke };

        // TODO: can overlapping tags break this?
        // PDF can only represent one text decoration style at a time.
        // If PDF/UA-1 is enforced throw an error.
        if options.is_pdf_ua()
            && self
                .items
                .iter()
                .filter_map(|(_, a)| a.as_deco())
                .any(|d| d.kind != deco.kind)
        {
            let validator = options.standards.config.validator().as_str();
            bail!(
                elem.span(),
                "{validator} error: cannot combine underline, overline, or strike"
            );
        }

        self.push(elem, TextAttr::Deco(deco));
        Ok(())
    }

    pub fn push(&mut self, elem: &Content, attr: TextAttr) {
        let loc = elem.location().unwrap();
        self.last_resolved = None;
        self.items.push((loc, attr));
    }

    /// Returns true if a decoration was removed.
    pub fn pop(&mut self, loc: Location) -> bool {
        self.last_resolved = None;

        // TODO: Ideally we would just check the top of the stack, can
        // overlapping tags even happen for decorations?
        if let Some(i) = self.items.iter().rposition(|(l, _)| *l == loc) {
            self.items.remove(i);
            return true;
        }
        false
    }

    pub fn resolve(&mut self, text: &TextItem) -> ResolvedTextAttrs {
        let params = TextParams::new(text);
        if let Some((prev_params, attrs)) = &self.last_resolved
            && prev_params == &params
        {
            return *attrs;
        }

        let attrs = resolve_attrs(&self.items, &text.font, text.size);
        self.last_resolved = Some((params, attrs));
        attrs
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextAttr {
    Strong,
    Emph,
    Script(Script),
    Highlight(Option<NaiveRgbColor>),
    Deco(TextDeco),
}

impl TextAttr {
    fn as_deco(&self) -> Option<&TextDeco> {
        if let Self::Deco(v) = self { Some(v) } else { None }
    }
}

/// Sub- or super-script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Script {
    kind: ScriptKind,
    baseline_shift: Smart<Length>,
    lineheight: Smart<TextSize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextDeco {
    kind: TextDecoKind,
    stroke: TextDecoStroke,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDecoKind {
    Underline,
    Overline,
    Strike,
}

impl TextDecoKind {
    fn to_krilla(self) -> TextDecorationType {
        match self {
            TextDecoKind::Underline => TextDecorationType::Underline,
            TextDecoKind::Overline => TextDecorationType::Overline,
            TextDecoKind::Strike => TextDecorationType::LineThrough,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TextDecoStroke {
    color: Option<NaiveRgbColor>,
    thickness: Option<Length>,
}

impl TextDecoStroke {
    fn from(stroke: Smart<Stroke>) -> Self {
        let Smart::Custom(stroke) = stroke else {
            return TextDecoStroke::default();
        };
        let color = stroke.paint.custom().as_ref().and_then(convert::paint_to_color);
        let thickness = stroke.thickness.custom();
        TextDecoStroke { color, thickness }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedTextAttrs {
    strong: Option<bool>,
    emph: Option<bool>,
    script: Option<ResolvedScript>,
    background: Option<Option<NaiveRgbColor>>,
    deco: Option<ResolvedTextDeco>,
}

impl ResolvedTextAttrs {
    pub const EMPTY: Self = Self {
        strong: None,
        emph: None,
        script: None,
        background: None,
        deco: None,
    };

    pub fn is_empty(&self) -> bool {
        self == &Self::EMPTY
    }

    pub fn all_resolved(&self) -> bool {
        self.strong.is_some()
            && self.emph.is_some()
            && self.script.is_some()
            && self.background.is_some()
            && self.deco.is_some()
    }

    pub fn resolve_nodes(self, accum: &mut Vec<Node>, children: Vec<kt::Identifier>) {
        enum Prev {
            Children(Vec<kt::Identifier>),
            Group(kt::TagGroup),
        }

        impl Prev {
            fn into_nodes(self) -> Vec<Node> {
                match self {
                    Prev::Children(ids) => ids.into_iter().map(Node::Leaf).collect(),
                    Prev::Group(group) => vec![Node::Group(group)],
                }
            }
        }

        let mut prev = Prev::Children(children);
        if self.script.is_some() || self.background.is_some() || self.deco.is_some() {
            let tag = Tag::Span
                .with_line_height(self.script.map(|s| s.lineheight))
                .with_baseline_shift(self.script.map(|s| s.baseline_shift))
                .with_background_color(self.background.flatten())
                .with_text_decoration_type(self.deco.map(|d| d.kind.to_krilla()))
                .with_text_decoration_color(self.deco.and_then(|d| d.color))
                .with_text_decoration_thickness(self.deco.and_then(|d| d.thickness));

            let group = kt::TagGroup::with_children(tag, prev.into_nodes());
            prev = Prev::Group(group);
        }
        if self.strong == Some(true) {
            let group = kt::TagGroup::with_children(Tag::Strong, prev.into_nodes());
            prev = Prev::Group(group);
        }
        if self.emph == Some(true) {
            let group = kt::TagGroup::with_children(Tag::Em, prev.into_nodes());
            prev = Prev::Group(group);
        }

        match prev {
            Prev::Group(group) => accum.push(Node::Group(group)),
            Prev::Children(ids) => accum.extend(ids.into_iter().map(Node::Leaf)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedScript {
    baseline_shift: f32,
    lineheight: LineHeight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedTextDeco {
    kind: TextDecoKind,
    color: Option<NaiveRgbColor>,
    thickness: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextParams {
    font_index: u32,
    size: Abs,
}

impl TextParams {
    fn new(text: &TextItem) -> TextParams {
        TextParams {
            // Comparing font indices is enough.
            font_index: text.font.index(),
            size: text.size,
        }
    }
}

fn resolve_attrs(
    items: &[(Location, TextAttr)],
    font: &Font,
    size: Abs,
) -> ResolvedTextAttrs {
    let mut attrs = ResolvedTextAttrs::EMPTY;
    for (_, attr) in items.iter().rev() {
        match *attr {
            TextAttr::Strong => {
                attrs.strong.get_or_insert(true);
            }
            TextAttr::Emph => {
                attrs.emph.get_or_insert(true);
            }
            TextAttr::Script(script) => {
                // TODO: The `typographic` setting is ignored for now.
                // Is it better to be accurate regarding the layouting, and
                // thus don't write any baseline shift and lineheight when
                // a typographic sub/super script glyph is used? Or should
                // we always write the shift so the sub/super script can be
                // picked up by AT?
                let script_metrics = script.kind.read_metrics(font.metrics());
                // NOTE: The user provided baseline_shift needs to be inverted.
                let baseline_shift = (script.baseline_shift.map(|s| -s.at(size)))
                    .unwrap_or_else(|| script_metrics.vertical_offset.at(size));
                let lineheight = (script.lineheight.map(|s| s.0.at(size)))
                    .unwrap_or_else(|| script_metrics.height.at(size));

                attrs.script.get_or_insert_with(|| ResolvedScript {
                    baseline_shift: baseline_shift.to_f32(),
                    lineheight: LineHeight::Custom(lineheight.to_f32()),
                });
            }
            TextAttr::Highlight(color) => {
                attrs.background.get_or_insert(color);
            }
            TextAttr::Deco(TextDeco { kind, stroke }) => {
                attrs.deco.get_or_insert_with(|| {
                    let thickness = stroke.thickness.map(|t| t.at(size).to_f32());
                    ResolvedTextDeco { kind, color: stroke.color, thickness }
                });
            }
        }

        if attrs.all_resolved() {
            break;
        }
    }
    attrs
}
