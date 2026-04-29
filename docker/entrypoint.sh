#!/bin/sh
# ----------------------------------------------------------------------------
# Fennec container entrypoint.
#
# The runtime image ships the in-tree skills/ at /opt/fennec/skills.
# FENNEC_HOME (default /data) is a volume that persists across container
# recreation — but is empty on first boot. We seed skills into the volume
# so the agent's `SkillsLoader::load_from_directory(home/skills)` actually
# finds something on a fresh install.
#
# We never overwrite an existing skills/ directory: the operator may have
# customised it, or a previous container may have shipped a newer set. The
# image-skills-as-defaults model is intentional — to upgrade the bundled
# skills, an operator can `rm -rf /data/skills` and restart, or selectively
# copy entries from /opt/fennec/skills.
#
# After seeding, exec the fennec binary with whatever args were passed
# (defaulting to `gateway --host 0.0.0.0 --port 3000` per the Dockerfile
# CMD).
# ----------------------------------------------------------------------------
set -eu

FENNEC_HOME="${FENNEC_HOME:-/data}"

if [ ! -d "${FENNEC_HOME}/skills" ] && [ -d /opt/fennec/skills ]; then
    cp -r /opt/fennec/skills "${FENNEC_HOME}/skills"
fi

exec /usr/local/bin/fennec "$@"
