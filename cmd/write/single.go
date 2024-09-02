package write

import (
	"fmt"

	"github.com/spf13/cobra"
)

var streamName string
var events string
var eventType string

// readCmd represents the read command
var singleCmd = &cobra.Command{
	Use:   "single",
	Short: "Send a single write request to the database",
	Long: `Send a single write request to the database.
	Example:
	$ ./yapper write single --stream 'test-stream' --event-data '{"Data":"This is data."}' --type json`,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("single write called")
	},
}

func init() {
	// rootCmd.AddCommand(readCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// readCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// readCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")

	singleCmd.Flags().StringVarP(&streamName, "stream", "s", "", "Name of the stream to write to")
	singleCmd.Flags().StringVarP(&events, "event-data", "e", "", "The event data to write to the stream")
	singleCmd.Flags().StringVarP(&eventType, "type", "t", "", "Event type for the new event")
	singleCmd.Flags().StringVarP(&eventType, "file", "f", "", "Write event from file")
}
