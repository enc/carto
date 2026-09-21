# No go.mod/package.json/pyproject.toml/setup.py here -- this
# directory is only recognized as a component because .carto/roots.json
# declares it explicitly, exercising the declared-root override path
# (ADR-0034) rather than marker auto-detection.


def handler(event, context):
    return process(event)


def process(event):
    return event
