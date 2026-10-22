// Shared presentation components (P1): status banner, issue lines, paths, sizes, badges, disclosures,
// empty states. Pages compose these instead of writing their own versions.

export { statusBanner, stateLine, BANNER_WORD, type BannerKind, type BannerOpts } from './status';
export { issueLine, issueList } from './issue';
export { pathText, type PathOpts } from './path';
export { sizeEl, gateBadge, GATE_BADGE, learnMore, techBlock, emptyBlock, type GateBadge } from './bits';
