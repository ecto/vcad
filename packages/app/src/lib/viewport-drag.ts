// Shared drag state for the 3D viewport. Tracks whether the last pointer
// gesture moved far enough to count as a drag, so click handlers can ignore
// the click that follows a drag. Without this, rotating the camera on touch
// devices also selects whichever mesh the finger happened to land on.

const isCoarsePointer =
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(pointer: coarse)").matches;

const DRAG_THRESHOLD_PX = isCoarsePointer ? 8 : 4;
const DRAG_THRESHOLD_SQ = DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;

const state = {
  startX: 0,
  startY: 0,
  down: false,
  wasDrag: false,
};

export function viewportPointerDown(x: number, y: number) {
  state.startX = x;
  state.startY = y;
  state.down = true;
  state.wasDrag = false;
}

export function viewportPointerMove(x: number, y: number) {
  if (!state.down || state.wasDrag) return;
  const dx = x - state.startX;
  const dy = y - state.startY;
  if (dx * dx + dy * dy > DRAG_THRESHOLD_SQ) {
    state.wasDrag = true;
  }
}

export function viewportPointerUp() {
  state.down = false;
}

export function viewportWasDrag(): boolean {
  return state.wasDrag;
}
