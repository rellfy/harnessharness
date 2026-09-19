UPDATE message
SET content = json_array(json_object('type', 'text', 'text', content));
