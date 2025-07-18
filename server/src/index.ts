import express from 'express';
import { setRoutes } from './routes/filesystemRoutes';
import { AppDataSource } from './data-source';
import { User } from './entities/User';

const app = express();
const PORT = process.env.PORT || 3000;

// Middleware
app.use(express.json());

// Set up routes
setRoutes(app);

app.listen(PORT, () => {
    console.log(`Server is running on http://localhost:${PORT}`);
});


// initializing the db
async function db() {
  try {
    await AppDataSource.initialize();
    console.log("Data Source has been initialized and DB schema created.");

    const userRepo = AppDataSource.getRepository(User);
    const exists = await userRepo.findOneBy({ username: "admin" });

    if (!exists) {
      const admin = userRepo.create({
        username: "admin",
        password: "admin",
      });

      await userRepo.save(admin);
    }

  } catch (error) {
    console.error("Error during Data Source initialization:", error);
  }
}

db();