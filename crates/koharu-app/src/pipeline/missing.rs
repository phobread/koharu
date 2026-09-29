//! "Process only what's missing": for one page and one step, what the step may
//! still fill in without touching work that is already there.
//!
//! A full run redoes everything: detection replaces every text box (and with
//! it the OCR text, translations and hand edits), OCR re-reads every box, the
//! font detector resets every box's style and the translator rewrites every
//! translation. In only-missing mode a step runs only where its output is
//! absent, and the text steps only on the boxes that lack their field, so
//! finished pages and hand-corrected boxes are left alone; a box that already
//! has a translation (or, for fonts, a style) was filled in by hand and gets
//! no OCR or font prediction either. Nothing is judged
//! stale: a cleaned page stays cleaned even if boxes were added afterwards;
//! redoing a step stays a deliberate per-page action.

use koharu_core::{ImageRole, MaskRole, NodeId, NodeKind, Page, TextData};

use super::artifacts::Artifact;

/// What a step should do on a page in an only-missing run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Work {
    /// Everything this step produces is already there.
    Skip,
    /// Run the step on the whole page.
    Page,
    /// Run the step, but only these text boxes may change.
    Nodes(Vec<NodeId>),
}

impl Work {
    pub fn is_skip(&self) -> bool {
        matches!(self, Work::Skip)
    }
}

/// What a step producing `produces` still has to do on `page`. `changed`
/// says whether an earlier step of this run already changed the page, which
/// makes its render out of date.
pub(crate) fn missing_work(produces: &[Artifact], page: &Page, changed: bool) -> Work {
    let has = |artifact: Artifact| produces.contains(&artifact);
    if has(Artifact::TextBoxes) {
        // A segment mask means detection already ran, even if the user has
        // since deleted every box: don't bring those boxes back.
        let detected = Artifact::TextBoxes.ready(page) || has_mask(page, MaskRole::Segment);
        return if detected { Work::Skip } else { Work::Page };
    }
    if has(Artifact::OcrText) {
        // `None` = never read (OCR that found nothing stores an empty
        // string). A box with a translation was filled in by hand; it keeps
        // its empty OCR field.
        return nodes_where(page, |t| t.text.is_none() && t.translation.is_none());
    }
    if has(Artifact::Translations) {
        // Boxes with text but no translation yet. An emptied translation is
        // `Some("")`, a deliberate choice, and stays.
        return nodes_where(page, |t| {
            t.text.as_ref().is_some_and(|s| !s.trim().is_empty()) && t.translation.is_none()
        });
    }
    if has(Artifact::FontPredictions) {
        // The font detector also resets the box's style, and a prediction
        // changes how a finished box renders: only blank boxes get one.
        return nodes_where(page, |t| {
            t.font_prediction.is_none() && t.style.is_none() && t.translation.is_none()
        });
    }
    if has(Artifact::FinalRender) || has(Artifact::RenderedSprites) {
        let rendered = has_image(page, ImageRole::Rendered);
        return if changed || !rendered {
            Work::Page
        } else {
            Work::Skip
        };
    }
    let present = produces.iter().all(|&artifact| artifact.ready(page));
    if present { Work::Skip } else { Work::Page }
}

/// Whether any of `steps` (each given by what it produces) has work on
/// `page` before anything runs. Pages without any are left out of the run.
pub(crate) fn page_needs_work(steps: &[&[Artifact]], page: &Page) -> bool {
    steps
        .iter()
        .any(|produces| !missing_work(produces, page, false).is_skip())
}

fn nodes_where(page: &Page, predicate: impl Fn(&TextData) -> bool) -> Work {
    let ids: Vec<NodeId> = page
        .nodes
        .iter()
        .filter_map(|(id, node)| match &node.kind {
            NodeKind::Text(text) if predicate(text) => Some(*id),
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        Work::Skip
    } else {
        Work::Nodes(ids)
    }
}

fn has_mask(page: &Page, role: MaskRole) -> bool {
    page.nodes
        .values()
        .any(|n| matches!(&n.kind, NodeKind::Mask(mask) if mask.role == role))
}

fn has_image(page: &Page, role: ImageRole) -> bool {
    page.nodes
        .values()
        .any(|n| matches!(&n.kind, NodeKind::Image(image) if image.role == role))
}

#[cfg(test)]
mod tests {
    use koharu_core::{
        BlobRef, FontPrediction, ImageData, MaskData, Node, NodeKind, Page, TextData, Transform,
    };

    use super::*;

    const DETECT: &[Artifact] = &[Artifact::TextBoxes];
    const SEGMENT: &[Artifact] = &[Artifact::SegmentMask];
    const BUBBLES: &[Artifact] = &[Artifact::BubbleMask];
    const FONTS: &[Artifact] = &[Artifact::FontPredictions];
    const OCR: &[Artifact] = &[Artifact::OcrText];
    const TRANSLATE: &[Artifact] = &[Artifact::Translations];
    const INPAINT: &[Artifact] = &[Artifact::Inpainted];
    const RENDER: &[Artifact] = &[Artifact::FinalRender, Artifact::RenderedSprites];
    const PROCESS: [&[Artifact]; 8] = [
        DETECT, SEGMENT, BUBBLES, FONTS, OCR, TRANSLATE, INPAINT, RENDER,
    ];

    fn node(kind: NodeKind) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform::default(),
            visible: true,
            kind,
        }
    }

    fn text(text: Option<&str>, translation: Option<&str>, font: bool) -> Node {
        node(NodeKind::Text(TextData {
            text: text.map(str::to_string),
            translation: translation.map(str::to_string),
            font_prediction: font.then(FontPrediction::default),
            ..Default::default()
        }))
    }

    fn styled(node: Node) -> Node {
        let mut node = node;
        if let NodeKind::Text(text) = &mut node.kind {
            text.style = Some(Default::default());
        }
        node
    }

    fn image(role: ImageRole) -> Node {
        node(NodeKind::Image(ImageData {
            role,
            blob: BlobRef::new("x"),
            opacity: 1.0,
            natural_width: 10,
            natural_height: 10,
            name: None,
        }))
    }

    fn mask(role: MaskRole) -> Node {
        node(NodeKind::Mask(MaskData {
            role,
            blob: BlobRef::new("x"),
        }))
    }

    fn page(nodes: Vec<Node>) -> Page {
        let mut page = Page::new("p", 10, 10);
        page.nodes = nodes.into_iter().map(|n| (n.id, n)).collect();
        page
    }

    /// A page the full pipeline has finished, boxes proofread.
    fn finished() -> Page {
        page(vec![
            image(ImageRole::Source),
            mask(MaskRole::Segment),
            mask(MaskRole::Bubble),
            text(Some("원문"), Some("done"), true),
            image(ImageRole::Inpainted),
            image(ImageRole::Rendered),
        ])
    }

    #[test]
    fn a_new_page_needs_every_step() {
        let fresh = page(vec![image(ImageRole::Source)]);
        assert!(page_needs_work(&PROCESS, &fresh));
        assert_eq!(missing_work(DETECT, &fresh, false), Work::Page);
        assert_eq!(missing_work(INPAINT, &fresh, false), Work::Page);
        assert_eq!(missing_work(RENDER, &fresh, false), Work::Page);
    }

    #[test]
    fn a_finished_page_is_left_alone() {
        let done = finished();
        assert!(!page_needs_work(&PROCESS, &done));
        for step in PROCESS {
            assert_eq!(missing_work(step, &done, false), Work::Skip, "{step:?}");
        }
    }

    #[test]
    fn detection_does_not_return_after_the_user_deleted_every_box() {
        let cleared = page(vec![image(ImageRole::Source), mask(MaskRole::Segment)]);
        assert_eq!(missing_work(DETECT, &cleared, false), Work::Skip);
    }

    #[test]
    fn text_steps_only_touch_the_boxes_missing_their_field() {
        let proofread = text(Some("고친 글"), Some("fixed"), true);
        let new_box = text(None, None, false);
        let blank_ocr = text(Some(""), None, true);
        let emptied = text(Some("효과음"), Some(""), true);
        let untranslated = text(Some("대사"), None, true);
        let ids = [new_box.id, untranslated.id];
        let p = page(vec![proofread, new_box, blank_ocr, emptied, untranslated]);

        assert_eq!(missing_work(OCR, &p, false), Work::Nodes(vec![ids[0]]));
        assert_eq!(missing_work(FONTS, &p, false), Work::Nodes(vec![ids[0]]));
        // The new box has no text yet, the blank one has nothing to translate
        // and the emptied translation was a choice.
        assert_eq!(
            missing_work(TRANSLATE, &p, false),
            Work::Nodes(vec![ids[1]])
        );
    }

    #[test]
    fn boxes_filled_in_by_hand_are_left_alone() {
        // A box drawn for a sound effect, translation typed in, font set.
        let hand_sfx = styled(text(None, Some("THERE~"), false));
        // Same without a style: a prediction would change how it renders.
        let hand_plain = text(None, Some("SLRRPP"), false);
        // Styled by hand before OCR: gets read, but keeps its style.
        let styled_new = styled(text(None, None, false));
        let styled_id = styled_new.id;
        let p = page(vec![hand_sfx, hand_plain, styled_new]);

        assert_eq!(missing_work(OCR, &p, false), Work::Nodes(vec![styled_id]));
        assert_eq!(missing_work(FONTS, &p, false), Work::Skip);
        assert_eq!(missing_work(TRANSLATE, &p, false), Work::Skip);
    }

    #[test]
    fn render_follows_changes_made_in_the_same_run() {
        let done = finished();
        assert_eq!(missing_work(RENDER, &done, false), Work::Skip);
        assert_eq!(missing_work(RENDER, &done, true), Work::Page);
    }

    #[test]
    fn a_cleaned_page_is_not_cleaned_again() {
        // Boxes added after cleaning still get OCR, but the cleaned layer
        // (with any hand repairs) is kept.
        let mut p = finished();
        let added = text(None, None, false);
        p.nodes.insert(added.id, added);
        assert_eq!(missing_work(INPAINT, &p, false), Work::Skip);
        assert!(page_needs_work(&PROCESS, &p));
    }
}
