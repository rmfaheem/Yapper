package write

import (
	"fmt"

	"github.com/spf13/cobra"
)

var clients int
var streams int
var requests int
var streamPrefix string
var eventSize int
var batchSize int

// readCmd represents the read command
var floodCmd = &cobra.Command{
	Use:   "flood",
	Short: "Send a flood of write requests to the database",
	Long: `Send a flood of write requests to the database.
	Example:
	$ ./yapper write flood --clients 5 --requests 10 --streams 100 --event-size 50 --batch-size 10 --stream-prefix yap`,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("flood write called")
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

	floodCmd.Flags().IntVarP(&clients, "clients", "c", 1, "Number of clients")
	floodCmd.Flags().IntVarP(&requests, "requests", "r", 1, "Number of write requests per client")
	floodCmd.Flags().IntVarP(&streams, "streams", "s", 1, "Number of streams per client")
	floodCmd.Flags().IntVarP(&eventSize, "event-size", "e", 10, "Average event size in bytes")
	floodCmd.Flags().IntVarP(&batchSize, "batch-size", "b", 1, "Batch size per request")
	floodCmd.Flags().StringVarP(&streamPrefix, "stream-prefix", "p", "", "Prefix for all streams generated via this command")
}
