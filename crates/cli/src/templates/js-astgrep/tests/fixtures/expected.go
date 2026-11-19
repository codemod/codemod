package main

import (
	"fmt"
	"errors"
)

func main() {
	// These fmt calls will be matched and replaced
	log.Info("message", "args", []interface{}{"Hello, World!"})
	log.Infof("User %s logged in
", "john")
	
	name := "Alice"
	age := 30
	log.Info("message", "args", []interface{}{"User info:", name, age})
	
	if err := doSomething(); err != nil {
		fmt.Printf("Error occurred: %v
", err)
	}
	
	// This should not be matched (string literal)
	message := "fmt.Println should not match this"
	
	// This should be matched
	log.Info("message", "args", []interface{}{"Processing complete"})
}

func doSomething() error {
	log.Infof("Starting operation at %s
", "now")
	return errors.New("something went wrong")
}
