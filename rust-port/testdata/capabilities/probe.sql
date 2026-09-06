CREATE TABLE widgets (id INT, name TEXT);

CREATE FUNCTION render(name TEXT) RETURNS TEXT AS $$
  SELECT help(name);
$$ LANGUAGE SQL;

SELECT render(name) FROM widgets;
