package src

import (
	"encoding/json"
	"fmt"
)

func PrettyJSON(data string) (string, error) {
	// Unmarshal the string into a generic map
	prettyData, err := PrettyJSONObject(data)
	if err != nil {
		return "", err
	}

	// Marshal with indentation to get pretty JSON
	prettyBytes, err := json.MarshalIndent(prettyData, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to marshal data string: %w", err)
	}
	return string(prettyBytes), nil
}

func PrettyJSONObject(data string) (prettyData map[string]any, err error) {
	// Unmarshal the string into a generic map
	err = json.Unmarshal([]byte(data), &prettyData)
	if err != nil {
		return prettyData, fmt.Errorf("failed to unmarshal data string: %w", err)
	}
	return prettyData, err
}
