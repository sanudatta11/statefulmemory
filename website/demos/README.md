# HyperFrames demo sources for statefulmemory.dev videos.
# Rendered MP4s live in ../public/videos/.
#
# Preview:  cd statefulmemory-clips && npm run dev
# Check:    cd statefulmemory-clips && npm run check
# Render:   npx hyperframes render . -c compositions/overview.html -o ../../public/videos/overview-v2.mp4
#
# Clips:
#   overview          self-host · cloud SaaS walking tour
#   install-agents    agent wiring + verify hooks
#   save-search-context
#   decide-mem        Decide + mem archives
#   anchors-verify    stale withdrawal vs git HEAD
#   graph-briefing    entity graph: save → graph → context expansion
#   ui-tour           loopback web dashboard (search / graph / decide)
#   own-your-memory   hero — portable, inspectable, team grants
#
# Render each with:
#   npx hyperframes render . -c compositions/<name>.html -o ../../public/videos/<name>-v2.mp4
# Poster (scene-midpoint):
#   ffmpeg -ss <t> -i ../../public/videos/<name>-v2.mp4 -frames:v 1 -q:v 3 ../../public/videos/<name>-v2-poster.jpg
#
