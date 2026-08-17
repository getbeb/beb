#!/bin/sh
# beb-ssh was split into a depot and a courier. This stays so the old
# command says where they went rather than 404ing at whoever wrote it down.
echo "beb-ssh became two programs:" >&2
echo "  curl -fsSL https://getbeb.dev/courier.sh | sh   # on each machine with mail" >&2
echo "  curl -fsSL https://getbeb.dev/depot.sh | sh     # on the one they can all reach" >&2
exit 1
