package subscribe

import (
	"fmt"

	"github.com/spf13/cobra"
)

// readCmd represents the read command
var persistentCmd = &cobra.Command{
	Use:   "persistent",
	Short: "Subscribe to a persistent subscription",
	Long: `Subscribe to the database using Persistent Subscriptions.
	All subscriptions will be automatically deleted after the command is run,
	unless the do-not-delete flag is used.
	
	Example:
	$`,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("persistent subscription called")
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
}
