#ifndef MIGHTTY_GHOSTTY_SEARCH_H
#define MIGHTTY_GHOSTTY_SEARCH_H

#include <ghostty/vt.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MighttyGhosttySearchImpl* MighttyGhosttySearch;

typedef enum {
  MIGHTTY_GHOSTTY_SEARCH_STEP_PENDING = 0,
  MIGHTTY_GHOSTTY_SEARCH_STEP_COMPLETE = 1,
  MIGHTTY_GHOSTTY_SEARCH_STEP_MAX_VALUE = GHOSTTY_ENUM_MAX_VALUE,
} MighttyGhosttySearchStep;

typedef enum {
  MIGHTTY_GHOSTTY_SEARCH_DIRECTION_NEXT = 0,
  MIGHTTY_GHOSTTY_SEARCH_DIRECTION_PREVIOUS = 1,
  MIGHTTY_GHOSTTY_SEARCH_DIRECTION_MAX_VALUE = GHOSTTY_ENUM_MAX_VALUE,
} MighttyGhosttySearchDirection;

typedef struct {
  uint16_t start_x;
  uint32_t start_y;
  uint16_t end_x;
  uint32_t end_y;
} MighttyGhosttySearchRange;

GhosttyResult mightty_ghostty_search_new(
    GhosttyTerminal terminal,
    const uint8_t* query,
    size_t query_len,
    MighttyGhosttySearch* out_search);

void mightty_ghostty_search_free(MighttyGhosttySearch search);

GhosttyResult mightty_ghostty_search_step(
    MighttyGhosttySearch search,
    MighttyGhosttySearchStep* out_step);

GhosttyResult mightty_ghostty_search_ranges(
    MighttyGhosttySearch search,
    MighttyGhosttySearchRange* ranges,
    size_t capacity,
    size_t* out_len);

GhosttyResult mightty_ghostty_search_select(
    MighttyGhosttySearch search,
    MighttyGhosttySearchDirection direction,
    MighttyGhosttySearchRange* out_range);

#ifdef __cplusplus
}
#endif

#endif
