---
refactor: arch-deepening-2026-07
invariant-class: refactor     # Rule 0 answered: behavior preserved, structural deepening, no metric, not breadth
entry-track: architecture     # Rule 0 answered: behavior does NOT change
review-track: full
pipeline-stage: intake        # intake | discover | design | ...
issue-tracker: local
candidate:                    # set at discover: the picked deepening candidate (one line)
intake-grill:                 # "done" after discover's grilling — design runs capture-only
spike-1:                      # <path>@pending | @done | @deleted
---

## Track note

사용자 요청: "전체적으로 아키텍쳐 및 성능 등 개선할점을 찾고 진행하자" — 후보 미정
상태로 architecture 트랙 진입. discovery(improve-codebase-architecture)가 deletion-test
후보들을 제시하고 인간이 하나를 고른다. 고른 후보가 선언 가능한 beatable metric을
가지면 그 시점에 gated-perf로 재라우팅한다(무metric이면 이 파이프라인 유지).
