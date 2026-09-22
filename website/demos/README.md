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
# Film assets (from brag-output-2026-09-22-152850/):
#   brag.mp4          120s product film (poster baked as frame 0) → public/videos/brag.mp4
#   brag.jpg          film poster → public/videos/brag-poster.jpg and public/og.jpg
#   laya-mlx-v1.mp4   Laya chapter cut (ch6, ~82–100s) → public/videos/laya-mlx-v1.mp4
#
# Re-cut the Laya clip from the master film:
#   ffmpeg -ss 81.8 -i brag.mp4 -t 18.6 -c:v libx264 -crf 23 -pix_fmt yuv420p \
#     -c:a aac -b:a 128k -movflags +faststart public/videos/laya-mlx-v1.mp4
#
# Render each with:
#   npx hyperframes render . -c compositions/<name>.html -o ../../public/videos/<name>-v2.mp4
# Poster (scene-midpoint):
#   ffmpeg -ss <t> -i ../../public/videos/<name>-v2.mp4 -frames:v 1 -q:v 3 ../../public/videos/<name>-v2-poster.jpg
#
