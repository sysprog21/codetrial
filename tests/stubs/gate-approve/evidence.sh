#!/bin/sh
# Stand-in for gate-evidence.sh when a test is about something else: logs the
# call and answers $FAKE_EVIDENCE (pass, ask or anything else for
# indeterminate).
echo "EVIDENCE $*" >> "$FAKE_LOG"
case ${FAKE_EVIDENCE:-pass} in
    pass)
        echo '{"verdict":"pass","reason":"gate evidence holds","head":"x"}'
        ;;
    ask)
        echo '{"verdict":"ask","reason":"no receipt","head":"x"}'
        exit 1
        ;;
    *)
        echo '{"verdict":"indeterminate","reason":"canned","head":"x"}'
        exit 2
        ;;
esac
