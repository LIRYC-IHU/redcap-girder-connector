# REDCap for the e2e stack: the user's licensed image, unchanged except for PHP overrides.
# The image already carries the database.php getenv() rewrite (configure-db.sh) and the
# entrypoint from ~/Code/docker-redcap; the fresh-database install is done by
# ../provision.sh through REDCap's own install.php?auto=1.
FROM docker-redcap-php:latest

COPY e2e-php.ini $PHP_INI_DIR/conf.d/zz-e2e.ini
