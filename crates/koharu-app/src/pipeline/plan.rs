//! Run planning for [`super::run`]: the order its (page, step) units execute
//! in, and which steps a failure rules out. Free of engines and sessions so it
//! can be tested directly.

use std::collections::HashSet;

use super::artifacts::Artifact;

/// What one step reads and writes, as declared by its engine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StepIo<'a> {
    pub needs: &'a [Artifact],
    pub produces: &'a [Artifact],
}

/// Per page, the artifacts that failed or skipped steps never produced.
///
/// A step runs only if none of its inputs are missing, so a page whose
/// translation failed is still inpainted; only its render is skipped.
#[derive(Debug, Default, Clone)]
pub(crate) struct MissingArtifacts(HashSet<Artifact>);

impl MissingArtifacts {
    /// Whether `step` needs something an earlier step failed to produce.
    pub fn blocks(&self, step: StepIo<'_>) -> bool {
        step.needs.iter().any(|artifact| self.0.contains(artifact))
    }

    /// `step` failed or was skipped, so nothing it produces can be relied on.
    pub fn record(&mut self, step: StepIo<'_>) {
        self.0.extend(step.produces.iter().copied());
    }
}

/// `(page_index, step_index)` units in execution order. Pages go through the
/// steps in chunks of `chunk`: every page of a chunk finishes a step before
/// any starts the next, so each engine loads once per chunk. `1` runs page by
/// page; a chunk as large as the run goes stage by stage.
pub(crate) fn schedule(pages: usize, steps: usize, chunk: usize) -> Vec<(usize, usize)> {
    let chunk = chunk.max(1);
    let mut units = Vec::with_capacity(pages * steps);
    for first in (0..pages).step_by(chunk) {
        let chunk_pages = first..(first + chunk).min(pages);
        for step in 0..steps {
            units.extend(chunk_pages.clone().map(|page| (page, step)));
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::Artifact::*;
    use super::*;

    // Declared inputs/outputs of the engines in a full Process run.
    const DETECT: StepIo<'static> = StepIo {
        needs: &[],
        produces: &[TextBoxes],
    };
    const SEGMENT: StepIo<'static> = StepIo {
        needs: &[TextBoxes],
        produces: &[SegmentMask],
    };
    const BUBBLES: StepIo<'static> = StepIo {
        needs: &[],
        produces: &[BubbleMask],
    };
    const FONTS: StepIo<'static> = StepIo {
        needs: &[TextBoxes],
        produces: &[FontPredictions],
    };
    const OCR: StepIo<'static> = StepIo {
        needs: &[TextBoxes],
        produces: &[OcrText],
    };
    const TRANSLATE: StepIo<'static> = StepIo {
        needs: &[OcrText],
        produces: &[Translations],
    };
    const INPAINT: StepIo<'static> = StepIo {
        needs: &[SegmentMask, BubbleMask],
        produces: &[Inpainted],
    };
    const RENDER: StepIo<'static> = StepIo {
        needs: &[Inpainted, Translations, FontPredictions],
        produces: &[FinalRender, RenderedSprites],
    };
    const PROCESS: [StepIo<'static>; 8] = [
        DETECT, SEGMENT, BUBBLES, FONTS, OCR, TRANSLATE, INPAINT, RENDER,
    ];

    /// Which steps of one page succeed when step `failed` fails.
    fn succeeded(failed: usize) -> Vec<bool> {
        let mut missing = MissingArtifacts::default();
        PROCESS
            .iter()
            .enumerate()
            .map(|(index, &step)| {
                let ok = index != failed && !missing.blocks(step);
                if !ok {
                    missing.record(step);
                }
                ok
            })
            .collect()
    }

    #[test]
    fn failed_translation_still_inpaints_and_skips_only_the_render() {
        assert_eq!(
            succeeded(5),
            [true, true, true, true, true, false, true, false]
        );
    }

    #[test]
    fn failed_detection_skips_everything_that_reads_text_boxes() {
        // Bubble segmentation doesn't read boxes; everything else does,
        // directly or through the segment mask, OCR text or translations.
        assert_eq!(
            succeeded(0),
            [false, false, true, false, false, false, false, false]
        );
    }

    #[test]
    fn failed_inpainting_or_bubbles_skip_the_render_only_where_needed() {
        assert_eq!(
            succeeded(6),
            [true, true, true, true, true, true, false, false]
        );
        assert_eq!(
            succeeded(2),
            [true, true, false, true, true, true, false, false]
        );
    }

    #[test]
    fn inputs_from_earlier_runs_do_not_block() {
        // Rendering alone relies on artifacts already on the page.
        assert!(!MissingArtifacts::default().blocks(RENDER));
    }

    #[test]
    fn chunk_of_one_finishes_each_page_before_the_next() {
        assert_eq!(
            schedule(2, 3, 1),
            [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]
        );
        assert!(schedule(0, 3, 1).is_empty());
        assert_eq!(schedule(2, 3, 0), schedule(2, 3, 1));
    }

    #[test]
    fn chunks_go_stage_by_stage_and_keep_each_pages_step_order() {
        assert_eq!(
            schedule(3, 2, 2),
            [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (2, 1)]
        );
        // A chunk covering the run: one pass per step.
        assert_eq!(schedule(2, 2, 8), [(0, 0), (1, 0), (0, 1), (1, 1)]);
        for chunk in 1..6 {
            let units = schedule(5, 4, chunk);
            assert_eq!(units.len(), 20);
            for page in 0..5 {
                let steps: Vec<_> = units.iter().filter(|u| u.0 == page).map(|u| u.1).collect();
                assert_eq!(steps, [0, 1, 2, 3], "chunk {chunk}, page {page}");
            }
        }
    }
}
