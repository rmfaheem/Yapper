package database

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/EventStore/EventStore-Client-Go/v4/esdb"
	"github.com/google/uuid"
	"github.com/rmfaheem/yapper/internal/config"
)

type TestEvent struct {
	Id            string
	ImportantData string
}

type Database struct {
	Client *esdb.Client
	Config *config.Config
}

func Init(config *config.Config) *Database {
	settings, err := esdb.ParseConnectionString((*config).BuildConnectionString())
	if err != nil {
		panic(err)
	}

	client, err := esdb.NewClient(settings)
	if err != nil {
		panic(err)
	}

	db := Database{
		client, config,
	}

	return &db
}

func (db *Database) Write(streamName string, eventType string, eventData string) *esdb.WriteResult {
	data := TestEvent{
		Id:            uuid.NewString(),
		ImportantData: eventData,
	}

	bytes, err := json.Marshal(data)
	if err != nil {
		panic(err)
	}

	options := esdb.AppendToStreamOptions{
		ExpectedRevision: esdb.Any{},
	}

	result, err := db.Client.AppendToStream(context.Background(), streamName, options, esdb.EventData{
		ContentType: esdb.ContentTypeJson,
		EventType:   eventType,
		Data:        bytes,
	})

	if err != nil {
		panic(err)
	}

	return result
}

func (db *Database) Wrfl(clients int, streamCount int, requests int, nodePreference string, eventSize int, batchSize int, streamPrefix string) *chan string {

	output := make(chan string)
	done := make(chan int)

	for j := 0; j < clients; j++ {

		for k := 0; k < streamCount; k++ {
			streamName := uuid.NewString()

			go func(index int) {
				output <- fmt.Sprintf("Appending events to stream: %s\n", streamName)
				for i := 0; i < requests; i++ {
					db.Write(
						streamPrefix+streamName,
						uuid.NewString(),
						uuid.NewString(),
					)
				}
				output <- fmt.Sprintf("Appended 10k events to stream: %v\n", streamName)
				done <- index
			}(k)
		}
	}

	total := 0
	for {
		count := <-done      // pull the latest loop index from the channel
		total += (count + 1) // the first index is 0, which would not increment the total, so we bump all the indexes by one

		// channel signals come in out of order
		// so i calculate the total to see if all the loops completed
		if total == (clients * streamCount) {
			break
		}
		time.Sleep(30 * time.Second)
	}

	close(done)
	output <- fmt.Sprintln("No more yap.")

	return &output
}
